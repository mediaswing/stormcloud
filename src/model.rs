//! Console tree nodes, their Graph endpoints and result-pane columns.

use crate::i18n::t;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Root,
    Users,
    Groups,
    Devices,
    Applications,
    RemoteServers,
    Exchange,
    Mailboxes,
    DistributionLists,
    Licences,
}

/// Where a node's list comes from.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// A Microsoft Graph collection.
    Graph(&'static str),
    /// Exchange Online cmdlets (see `exchange::list`).
    Exchange,
}

/// Order and depth of the console tree.
pub const TREE: &[(Node, usize)] = &[
    (Node::Root, 0),
    (Node::Users, 1),
    (Node::Groups, 1),
    (Node::Devices, 1),
    (Node::Applications, 1),
    (Node::RemoteServers, 1),
    (Node::Licences, 1),
    (Node::Exchange, 1),
    (Node::Mailboxes, 2),
    (Node::DistributionLists, 2),
];

pub struct Column {
    /// The key of the column's title in the catalogue (see `i18n`).
    pub title: &'static str,
    pub width: f32,
    pub get: fn(&Value) -> String,
}

impl Node {
    pub fn title(self) -> String {
        t(match self {
            Node::Root => "node.root",
            Node::Users => "node.users",
            Node::Groups => "node.groups",
            Node::Devices => "node.devices",
            Node::Applications => "node.applications",
            Node::RemoteServers => "node.remote_servers",
            Node::Exchange => "node.exchange",
            Node::Mailboxes => "node.mailboxes",
            Node::DistributionLists => "node.distribution_lists",
            Node::Licences => "node.licences",
        })
    }

    /// The objects listed under this node, as named in the middle of a sentence, such as "users"
    /// in "Loading users…".
    pub fn list_name(self) -> String {
        t(match self {
            Node::Users => "node.users_list",
            Node::Groups => "node.groups_list",
            Node::Devices => "node.devices_list",
            Node::Applications => "node.applications_list",
            Node::RemoteServers => "node.remote_servers_list",
            Node::Mailboxes => "node.mailboxes_list",
            Node::DistributionLists => "node.distribution_lists_list",
            Node::Licences => "node.licences_list",
            Node::Root | Node::Exchange => "node.items_list",
        })
    }

    pub fn icon(self) -> &'static str {
        match self {
            Node::Root => "🏢",
            Node::Users => "👤",
            Node::Groups => "👥",
            Node::Devices => "💻",
            Node::Applications => "🧩",
            Node::RemoteServers => "🖥",
            Node::Exchange => "📧",
            Node::Mailboxes => "📬",
            Node::DistributionLists => "📨",
            Node::Licences => "🎫",
        }
    }

    pub fn parent(self) -> Option<Node> {
        match self {
            Node::Root => None,
            Node::Mailboxes | Node::DistributionLists => Some(Node::Exchange),
            _ => Some(Node::Root),
        }
    }

    pub fn has_children(self) -> bool {
        matches!(self, Node::Root | Node::Exchange)
    }

    /// Whether the node lists objects, rather than the nodes under it.
    pub fn is_list(self) -> bool {
        self.endpoint().is_some() || self == Node::RemoteServers
    }

    pub fn endpoint(self) -> Option<Source> {
        Some(Source::Graph(match self {
            Node::Users => "/users?$select=id,displayName,userPrincipalName,mail,accountEnabled,userType,jobTitle,department,usageLocation,createdDateTime,assignedLicenses&$top=999",
            Node::Groups => "/groups?$select=id,displayName,description,mail,mailEnabled,mailNickname,securityEnabled,groupTypes,isAssignableToRole,membershipRule,createdDateTime,assignedLicenses&$top=999",
            Node::Devices => "/devices?$select=id,deviceId,displayName,operatingSystem,operatingSystemVersion,trustType,accountEnabled,approximateLastSignInDateTime,isCompliant,isManaged,registrationDateTime&$top=999",
            Node::Applications => "/servicePrincipals?$select=id,appId,displayName,publisherName,appOwnerOrganizationId,servicePrincipalType,accountEnabled,appRoleAssignmentRequired,tags,appRoles,homepage&$top=999",
            Node::Licences => "/subscribedSkus",
            Node::Mailboxes | Node::DistributionLists => return Some(Source::Exchange),
            // Remote Servers come from `servers.json` (see `servers`), not from Microsoft.
            Node::Root | Node::Exchange | Node::RemoteServers => return None,
        }))
    }

    pub fn columns(self) -> Vec<Column> {
        fn c(title: &'static str, width: f32, get: fn(&Value) -> String) -> Column {
            Column { title, width, get }
        }
        match self {
            Node::Users => vec![
                c("col.display_name", 200.0, |v| s(v, "displayName")),
                c("col.upn", 260.0, |v| s(v, "userPrincipalName")),
                c("col.enabled", 70.0, |v| yes_no(&v["accountEnabled"])),
                c("col.type", 80.0, |v| s(v, "userType")),
                c("col.job_title", 140.0, |v| s(v, "jobTitle")),
                c("col.department", 140.0, |v| s(v, "department")),
                c("col.licences", 70.0, |v| count(&v["assignedLicenses"])),
            ],
            Node::Groups => vec![
                c("col.display_name", 220.0, |v| s(v, "displayName")),
                c("col.type", 120.0, group_type),
                c("col.membership", 90.0, membership),
                c("col.role_assignable", 110.0, |v| yes_no(&v["isAssignableToRole"])),
                c("col.email", 220.0, |v| s(v, "mail")),
                c("col.description", 260.0, |v| s(v, "description")),
            ],
            Node::Devices => vec![
                c("col.name", 200.0, |v| s(v, "displayName")),
                c("col.os", 90.0, |v| s(v, "operatingSystem")),
                c("col.version", 120.0, |v| s(v, "operatingSystemVersion")),
                c("col.join_type", 130.0, |v| join_type(&s(v, "trustType"))),
                c("col.enabled", 70.0, |v| yes_no(&v["accountEnabled"])),
                c("col.compliant", 80.0, |v| yes_no(&v["isCompliant"])),
                c("col.managed", 80.0, |v| yes_no(&v["isManaged"])),
                c("col.last_sign_in", 140.0, |v| date(&v["approximateLastSignInDateTime"])),
            ],
            Node::Applications => vec![
                c("col.name", 220.0, |v| s(v, "displayName")),
                c("col.publisher", 180.0, |v| s(v, "publisherName")),
                c("col.enabled", 70.0, |v| yes_no(&v["accountEnabled"])),
                c("col.assignment_required", 130.0, |v| yes_no(&v["appRoleAssignmentRequired"])),
                c("col.visible_to_users", 110.0, |v| yes_no(&visible_to_users(v))),
                c("col.application_id", 280.0, |v| s(v, "appId")),
            ],
            Node::RemoteServers => vec![
                c("col.name", 180.0, |v| s(v, "name")),
                c("col.host", 180.0, |v| s(v, "host")),
                c("col.os", 190.0, crate::servers::os),
                c("col.kernel", 170.0, crate::servers::kernel),
                c("col.installed_apps", 90.0, crate::servers::package_count),
                c("col.running_services", 90.0, crate::servers::service_count),
                c("col.pending_updates", 130.0, crate::servers::patch_level),
                c("col.reboot_required", 90.0, crate::servers::reboot_required),
                c("col.last_checked", 130.0, crate::servers::last_checked),
                c("col.status", 200.0, crate::servers::status),
            ],
            Node::Licences => vec![
                c("col.product", 260.0, |v| sku_name(&s(v, "skuPartNumber"))),
                c("col.sku", 200.0, |v| s(v, "skuPartNumber")),
                c("col.assigned", 80.0, |v| v["consumedUnits"].as_i64().unwrap_or(0).to_string()),
                c("col.total", 80.0, |v| v["prepaidUnits"]["enabled"].as_i64().unwrap_or(0).to_string()),
                c("col.available", 80.0, |v| {
                    let total = v["prepaidUnits"]["enabled"].as_i64().unwrap_or(0);
                    (total - v["consumedUnits"].as_i64().unwrap_or(0)).to_string()
                }),
                c("col.status", 90.0, |v| s(v, "capabilityStatus")),
                c("col.applies_to", 90.0, |v| s(v, "appliesTo")),
            ],
            Node::Mailboxes => vec![
                c("col.display_name", 220.0, |v| s(v, "DisplayName")),
                c("col.email_address", 260.0, |v| s(v, "PrimarySmtpAddress")),
                c("col.type", 90.0, crate::exchange::mailbox_type),
                c("col.forwarding_to", 220.0, crate::exchange::forwarding),
                c("col.hidden", 70.0, |v| yes_no(&v["HiddenFromAddressListsEnabled"])),
            ],
            Node::DistributionLists => vec![
                c("col.display_name", 220.0, |v| s(v, "DisplayName")),
                c("col.email_address", 260.0, |v| s(v, "PrimarySmtpAddress")),
                c("col.type", 160.0, crate::exchange::group_type),
                c("col.internal_senders_only", 140.0, |v| yes_no(&v["RequireSenderAuthenticationEnabled"])),
                c("col.hidden", 70.0, |v| yes_no(&v["HiddenFromAddressListsEnabled"])),
            ],
            Node::Root | Node::Exchange => vec![
                c("col.name", 240.0, |v| s(v, "name")),
                c("col.description", 400.0, |v| s(v, "description")),
            ],
        }
    }

    /// Whether an object returned by [`Node::endpoint`] belongs in the list. Applications leaves
    /// out managed identities and Microsoft's own service principals, of which every tenant has
    /// hundreds, as the Entra admin centre's Enterprise applications list does.
    pub fn keep(self, v: &Value) -> bool {
        match self {
            Node::Applications => {
                v["servicePrincipalType"] == "Application"
                    && !MICROSOFT_TENANTS.contains(&s(v, "appOwnerOrganizationId").as_str())
            }
            _ => true,
        }
    }

    /// The label used to refer to an object of this node in dialogs.
    pub fn object_name(self, v: &Value) -> String {
        match self {
            Node::Licences => sku_name(&s(v, "skuPartNumber")),
            Node::Root | Node::Exchange | Node::RemoteServers => s(v, "name"),
            Node::Mailboxes | Node::DistributionLists => s(v, "DisplayName"),
            _ => s(v, "displayName"),
        }
    }
}

/// Tenants that own Microsoft's first-party applications.
const MICROSOFT_TENANTS: [&str; 2] = ["f8cdef31-a31e-4b4a-93e4-5f571e91255a", "72f988bf-86f1-41af-91ab-2d7cd011db47"];

/// Tag that hides an application from users' My Apps portal and the Microsoft 365 app launcher.
pub const HIDE_APP_TAG: &str = "HideApp";

pub fn visible_to_users(v: &Value) -> Value {
    match v["tags"].as_array() {
        Some(tags) => Value::Bool(!tags.iter().any(|t| t == HIDE_APP_TAG)),
        None => Value::Null,
    }
}

pub fn s(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn yes_no(v: &Value) -> String {
    match v.as_bool() {
        Some(true) => t("value.yes"),
        Some(false) => t("value.no"),
        None => String::new(),
    }
}

pub fn count(v: &Value) -> String {
    v.as_array().map(|a| a.len().to_string()).unwrap_or_default()
}

pub fn date(v: &Value) -> String {
    v.as_str().map(|d| d.get(..10).unwrap_or(d).to_string()).unwrap_or_default()
}

pub fn datetime(v: &Value) -> String {
    v.as_str()
        .map(|d| d.get(..16).unwrap_or(d).replace('T', " "))
        .unwrap_or_default()
}

pub fn group_type(v: &Value) -> String {
    let unified = v["groupTypes"]
        .as_array()
        .is_some_and(|a| a.iter().any(|t| t == "Unified"));
    let security = v["securityEnabled"].as_bool().unwrap_or(false);
    let mail = v["mailEnabled"].as_bool().unwrap_or(false);
    t(match (unified, security, mail) {
        (true, _, _) => "value.group_m365",
        (false, true, true) => "value.group_mail_security",
        (false, true, false) => "value.group_security",
        (false, false, true) => "value.group_distribution",
        _ => "value.group_other",
    })
}

pub fn membership(v: &Value) -> String {
    let dynamic = v["groupTypes"]
        .as_array()
        .is_some_and(|a| a.iter().any(|t| t == "DynamicMembership"));
    t(if dynamic { "value.dynamic" } else { "value.assigned" })
}

pub fn join_type(trust: &str) -> String {
    match trust {
        "AzureAd" => t("value.entra_joined"),
        "ServerAd" => t("value.hybrid_joined"),
        "Workplace" => t("value.entra_registered"),
        other => other.to_string(),
    }
}

/// Friendly product names for common SKU part numbers.
pub fn sku_name(part: &str) -> String {
    let name = match part {
        "ENTERPRISEPACK" => "Office 365 E3",
        "ENTERPRISEPREMIUM" => "Office 365 E5",
        "STANDARDPACK" => "Office 365 E1",
        "SPE_E3" => "Microsoft 365 E3",
        "SPE_E5" => "Microsoft 365 E5",
        "SPE_F1" => "Microsoft 365 F3",
        "SPB" => "Microsoft 365 Business Premium",
        "O365_BUSINESS_PREMIUM" => "Microsoft 365 Business Standard",
        "O365_BUSINESS_ESSENTIALS" => "Microsoft 365 Business Basic",
        "O365_BUSINESS" => "Microsoft 365 Apps for business",
        "OFFICESUBSCRIPTION" => "Microsoft 365 Apps for enterprise",
        "DEVELOPERPACK_E5" => "Microsoft 365 E5 Developer",
        "EMS" => "Enterprise Mobility + Security E3",
        "EMSPREMIUM" => "Enterprise Mobility + Security E5",
        "AAD_PREMIUM" => "Microsoft Entra ID P1",
        "AAD_PREMIUM_P2" => "Microsoft Entra ID P2",
        "INTUNE_A" => "Microsoft Intune Plan 1",
        "EXCHANGESTANDARD" => "Exchange Online (Plan 1)",
        "EXCHANGEENTERPRISE" => "Exchange Online (Plan 2)",
        "POWER_BI_PRO" => "Power BI Pro",
        "POWER_BI_STANDARD" => "Power BI (free)",
        "FLOW_FREE" => "Power Automate Free",
        "POWERAPPS_DEV" => "Power Apps Developer",
        "TEAMS_EXPLORATORY" => "Teams Exploratory",
        "Microsoft_Teams_Enterprise_New" => "Microsoft Teams Enterprise",
        "PROJECTPROFESSIONAL" => "Project Plan 3",
        "VISIOCLIENT" => "Visio Plan 2",
        "WIN_DEF_ATP" => "Microsoft Defender for Endpoint P2",
        "Microsoft_365_Copilot" => "Microsoft 365 Copilot",
        "WINDOWS_STORE" => "Windows Store for Business",
        _ => return part.replace('_', " "),
    };
    name.to_string()
}

/// Flattens a JSON object into display rows for a General property page: each label's key in the
/// catalogue, and the property it shows.
pub fn general_fields(v: &Value, keys: &[(&str, &str)]) -> Vec<(String, String)> {
    keys.iter()
        .map(|(label, key)| {
            let val = &v[*key];
            let text = match val {
                Value::Bool(_) => yes_no(val),
                Value::Array(a) => a
                    .iter()
                    .map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string()))
                    .collect::<Vec<_>>()
                    .join(", "),
                Value::String(x) if key.ends_with("DateTime") => datetime(&Value::String(x.clone())),
                _ => s(v, key),
            };
            (t(label), text)
        })
        .collect()
}
