//! Export of the result list to a table on a MariaDB (or MySQL) server.
//!
//! The table is created if it is missing, with a text column for each column shown and the
//! object's ID as the primary key. Columns added since an earlier export are added to the table.
//! Rows are upserted by ID, so exporting again updates the table in place; with "Replace" ticked,
//! rows for objects no longer in the list are removed too. Everything runs in one transaction.

use crate::app::App;
use crate::config::Settings;
use crate::dialogs::{Dialog, Field, Form};
use crate::i18n::{self, t, tf};
use crate::model::{Node, s};
use anyhow::Context;
use mysql::prelude::Queryable;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 3306;

/// Where and how to write an export.
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    pub table: String,
    pub tls: bool,
    /// Remove rows for objects that are not in this export.
    pub replace: bool,
}

/// A column name for a list header: lower case, with runs of anything but letters and digits
/// made a single underscore, so "Last sign-in" is `last_sign_in`.
pub fn column_name(header: &str) -> String {
    let mut out = String::new();
    for c in header.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// Table names are limited to letters, digits and underscores, so they never need escaping.
pub fn valid_table(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Checks that a table which already exists, with `existing` columns, was made by an export of
/// this kind, so a table belonging to something else is never altered.
fn check_existing(table: &str, existing: &[String], columns: &[String]) -> anyhow::Result<()> {
    let has = |name: &str| existing.iter().any(|c| c.eq_ignore_ascii_case(name));
    if existing.is_empty() || (has(&columns[0]) && has("exported_at")) {
        Ok(())
    } else {
        let key = if columns[0] == "sku_id" { "db.not_ours_licences" } else { "db.not_ours" };
        anyhow::bail!(tf(key, &[("table", &table), ("column", &columns[0])]))
    }
}

/// The statements that make `table` ready for `columns`, the first of which is the primary key.
fn schema_sql(table: &str, columns: &[String]) -> Vec<String> {
    let key = &columns[0];
    let mut sql = vec![format!(
        "CREATE TABLE IF NOT EXISTS `{table}` (`{key}` VARCHAR(64) NOT NULL PRIMARY KEY, \
         `exported_at` DATETIME(6) NOT NULL) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    )];
    sql.extend(columns[1..].iter().map(|c| format!("ALTER TABLE `{table}` ADD COLUMN IF NOT EXISTS `{c}` TEXT NULL")));
    sql
}

/// The upsert for one row, with a `?` per column and `exported_at` taken from `@exported_at`.
fn insert_sql(table: &str, columns: &[String]) -> String {
    let names: Vec<String> = columns.iter().map(|c| format!("`{c}`")).collect();
    let marks = vec!["?"; columns.len()].join(", ");
    let updates: Vec<String> =
        names.iter().skip(1).map(|c| format!("{c} = VALUES({c})")).chain(["`exported_at` = @exported_at".into()]).collect();
    format!(
        "INSERT INTO `{table}` ({}, `exported_at`) VALUES ({marks}, @exported_at) ON DUPLICATE KEY UPDATE {}",
        names.join(", "),
        updates.join(", ")
    )
}

/// Writes `rows` to the target table. `columns[0]` and each row's first cell are the object ID.
pub fn export(target: &Target, columns: &[String], rows: &[Vec<String>]) -> anyhow::Result<()> {
    let mut opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(&target.host))
        .tcp_port(target.port)
        .user(Some(&target.user))
        .pass(Some(&target.password))
        .db_name(Some(&target.database))
        .tcp_connect_timeout(Some(Duration::from_secs(15)))
        .read_timeout(Some(Duration::from_secs(120)))
        .write_timeout(Some(Duration::from_secs(120)));
    if target.tls {
        opts = opts.ssl_opts(mysql::SslOpts::default());
    }
    let mut conn = mysql::Conn::new(opts)
        .with_context(|| tf("db.connect_failed", &[("host", &target.host), ("port", &target.port)]))?;
    log::info!("Connected to MariaDB at {}:{} as {}", target.host, target.port, target.user);

    let table = &target.table;
    let existing: Vec<String> = conn
        .exec(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
            (table,),
        )
        .with_context(|| tf("db.lookup_failed", &[("table", table)]))?;
    check_existing(table, &existing, columns)?;
    // DDL commits implicitly in MariaDB, so the table is prepared before the transaction starts.
    for sql in schema_sql(table, columns) {
        log::debug!("SQL: {sql}");
        conn.query_drop(&sql).with_context(|| tf("db.prepare_failed", &[("table", table)]))?;
    }
    let mut tx = conn.start_transaction(mysql::TxOpts::default())?;
    tx.query_drop("SET @exported_at = UTC_TIMESTAMP(6)")?;
    tx.exec_batch(insert_sql(table, columns), rows.iter().cloned())
        .with_context(|| tf("db.write_failed", &[("table", table)]))?;
    if target.replace {
        tx.query_drop(format!("DELETE FROM `{table}` WHERE `exported_at` <> @exported_at"))
            .with_context(|| tf("db.remove_failed", &[("table", table)]))?;
    }
    tx.commit()?;
    Ok(())
}

impl App {
    /// Asks for the server and table, then saves the result list, as filtered on screen, to it.
    pub fn export_database(&mut self) {
        let node = self.node;
        let set = &self.settings;
        // Named in English, as the columns are, whatever the language of the interface.
        let table = format!("entra_{}", column_name(&i18n::in_english(|| node.title())));
        let form = Form::new(&t("db.title"), &t("db.export"))
            .intro(&t("db.intro"))
            .field(Field::text("host", &t("db.server"), &set.db_host).required())
            .field(Field::text("port", &t("db.port"), &set.db_port.to_string()).required())
            .field(Field::text("user", &t("field.user_name"), &set.db_user).required())
            .field(Field::password("password", &t("field.password"), ""))
            .field(Field::text("database", &t("db.database"), &set.db_name).required())
            .field(Field::text("table", &t("db.table"), &table).required())
            .field(Field::check("tls", &t("db.tls"), set.db_tls))
            .field(Field::check("replace", &t("db.replace"), false))
            .on_submit(move |app, f| {
                let port = f.text("port").parse::<u16>().ok().filter(|&p| p != 0);
                let Some(port) = port else {
                    return Err(t("db.bad_port"));
                };
                let table = f.text("table");
                if !valid_table(&table) {
                    return Err(t("db.bad_table"));
                }
                // Removing rows on an empty or partial list would delete objects that still exist.
                if f.checked("replace") {
                    if let Some(why) = app.incomplete_list(node) {
                        return Err(tf("db.incomplete", &[("why", &why)]));
                    }
                    if app.view_rows().is_empty() {
                        return Err(t("db.empty"));
                    }
                }
                let target = Target {
                    host: f.text("host"),
                    port,
                    user: f.text("user"),
                    password: f.raw("password"),
                    database: f.text("database"),
                    table,
                    tls: f.checked("tls"),
                    replace: f.checked("replace"),
                };
                let remembered = Settings {
                    db_host: target.host.clone(),
                    db_port: target.port,
                    db_user: target.user.clone(),
                    db_name: target.database.clone(),
                    db_tls: target.tls,
                    ..app.settings.clone()
                };
                if let Err(e) = remembered.save() {
                    log::warn!("Could not save the database settings: {e:#}");
                }
                app.settings = remembered;
                app.run_database_export(node, target);
                Ok(())
            });
        self.push(Dialog::Form(form));
    }

    fn run_database_export(&mut self, node: Node, target: Target) {
        let cols = node.columns();
        let (id_header, id_key) = if node == Node::Licences { ("SKU ID", "skuId") } else { ("Object ID", "id") };
        // The column names and values are in English whatever the language of the interface, so
        // the table reads the same, and exporting again updates it rather than adding columns.
        let (columns, rows) = i18n::in_english(|| {
            let columns: Vec<String> = std::iter::once(id_header.to_string())
                .chain(cols.iter().map(|c| t(c.title)))
                .map(|title| column_name(&title))
                .collect();
            let rows: Vec<Vec<String>> = self
                .view_rows()
                .into_iter()
                .map(|v| std::iter::once(s(v, id_key)).chain(cols.iter().map(|c| (c.get)(v))).collect())
                .collect();
            (columns, rows)
        });
        let list = node.list_name();
        let place = (target.database.clone(), target.table.clone(), target.host.clone());
        let args: [(&str, &dyn std::fmt::Display); 4] =
            [("list", &list), ("database", &place.0), ("table", &place.1), ("host", &place.2)];
        let label = tf("db.exporting", &args);
        self.spawn(
            label,
            move |_| export(&target, &columns, &rows).map(|()| rows.len()),
            move |app, r| match r {
                Ok(n) => {
                    log::info!("Exported {n} {node:?} to {}.{} on {}", place.0, place.1, place.2);
                    let args: [(&str, &dyn std::fmt::Display); 5] =
                        [("count", &n), ("list", &list), ("database", &place.0), ("table", &place.1), ("host", &place.2)];
                    app.status = tf("db.exported", &args);
                }
                Err(e) => app.error(&t("db.title"), format!("{e:#}")),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_from_headers() {
        assert_eq!(column_name("Display name"), "display_name");
        assert_eq!(column_name("Last sign-in"), "last_sign_in");
        assert_eq!(column_name("Role-assignable"), "role_assignable");
        assert_eq!(column_name("SKU ID"), "sku_id");
        assert_eq!(column_name("OS"), "os");
    }

    #[test]
    fn table_names() {
        assert!(valid_table("entra_users"));
        assert!(!valid_table("users; DROP TABLE x"));
        assert!(!valid_table("a`b"));
        assert!(!valid_table(""));
    }

    #[test]
    fn every_list_has_distinct_column_names() {
        for node in [Node::Users, Node::Groups, Node::Devices, Node::Applications, Node::Mailboxes, Node::DistributionLists, Node::Licences] {
            let mut names: Vec<String> = node.columns().iter().map(|c| column_name(i18n::english(c.title).unwrap())).collect();
            names.extend(["object_id".into(), "sku_id".into(), "exported_at".into()]);
            let count = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), count, "{node:?}");
        }
    }

    #[test]
    fn leaves_other_tables_alone() {
        let cols = vec!["object_id".to_string(), "display_name".to_string()];
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_existing("t", &[], &cols).is_ok());
        assert!(check_existing("t", &names(&["object_id", "exported_at", "other"]), &cols).is_ok());
        assert!(check_existing("t", &names(&["id", "name", "email"]), &cols).is_err());
        assert!(check_existing("t", &names(&["object_id", "name"]), &cols).is_err());
    }

    #[test]
    fn upsert_statement() {
        let cols = vec!["object_id".to_string(), "display_name".to_string()];
        assert_eq!(
            insert_sql("users", &cols),
            "INSERT INTO `users` (`object_id`, `display_name`, `exported_at`) VALUES (?, ?, @exported_at) \
             ON DUPLICATE KEY UPDATE `display_name` = VALUES(`display_name`), `exported_at` = @exported_at"
        );
        assert_eq!(schema_sql("users", &cols).len(), 2);
    }
}
