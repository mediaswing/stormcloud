# Graphical Cloud Management

A cross-platform desktop console for managing **Microsoft Entra ID**, laid out like a classic
MMC snap-in: console tree on the left, results list in the middle, Actions pane on the right,
with a menu bar, toolbar and status bar. Written in Rust with [egui/eframe](https://github.com/emilk/egui)
and talks to Microsoft Graph directly.

| Node | What you can do |
| --- | --- |
| **Users** | Create (one at a time or in bulk from CSV), edit, enable/disable, delete, reset password; manage group membership, licences and directory roles |
| **Groups** | Create (security / Microsoft 365, optionally role-assignable), edit, delete; manage members (also in bulk from CSV), owners, licences and **directory role assignments** |
| **Devices** | View details, owners, users and group membership; enable/disable; delete |
| **Applications** | View the enterprise applications connected to the tenant; enable/disable sign-in; delete; set whether assignment is required and whether users see them; manage the users and groups assigned, owners, and review or revoke the permissions granted to them |
| **Licences** | View subscriptions, service plans and licensed users; assign or remove licences |

## Setup

1. In the Entra admin centre, create an **app registration** (single tenant is fine).
2. Under **Authentication**, set **Allow public client flows** to **Yes**.
3. Under **API permissions**, add these *delegated* Microsoft Graph permissions and grant admin consent:
   `User.ReadWrite.All`, `Group.ReadWrite.All`, `Device.ReadWrite.All`, `Directory.Read.All`,
   `RoleManagement.ReadWrite.Directory`, `Organization.Read.All`, `LicenseAssignment.ReadWrite.All`,
   `User-PasswordProfile.ReadWrite.All`, `Application.ReadWrite.All`, `AppRoleAssignment.ReadWrite.All`,
   `DelegatedPermissionGrant.ReadWrite.All`.
4. Run Graphical Cloud Management. On first launch it asks for the tenant ID and client ID, then signs you in with a
   device code (it opens your browser for you).

The permissions are delegated: what you can actually change is still limited by your own Entra roles.

**Upgrading from 1.1.0:** the last three permissions are new, for the Applications node. After you
sign in, Graphical Cloud Management checks whether they have been granted:

- If an administrator has already added them to the app registration and granted admin consent,
  they are added to **Scopes** in `settings.ini` straight away, with nothing to approve.
- If not, it asks once whether to approve them now. Signing in with the code it shows lets a Global
  Administrator or Privileged Role Administrator tick **Consent on behalf of your organization**.
  If that is cancelled or refused, you stay signed in as before. You are not asked again
  (`app_scopes_offered = true` in `settings.ini`), but granting admin consent on the app registration
  later still adds them at the next sign-in.

Until they are granted you can view applications, but changing them fails with an "Insufficient
privileges" error.

## Settings

Settings live in `settings.ini` in the per-user configuration folder:

| OS | Path |
| --- | --- |
| Windows | `%APPDATA%\StormCloud\settings.ini` |
| macOS | `~/Library/Application Support/StormCloud/settings.ini` |
| Linux | `$XDG_CONFIG_HOME/stormcloud/settings.ini` (usually `~/.config/stormcloud`) |

```ini
[auth]
tenant_id = contoso.onmicrosoft.com
client_id = 00000000-0000-0000-0000-000000000000
remember_sign_in = false
scopes = User.ReadWrite.All Group.ReadWrite.All ...

[cloud]
authority_host = https://login.microsoftonline.com
graph_endpoint = https://graph.microsoft.com

[logging]
level = debug

[mariadb]
host = db.contoso.local
port = 3306
user = stormcloud
database = entra
tls = true
```

Edit it from **File › Settings…** or by hand. With `remember_sign_in = true` the refresh token is
saved to `token.json` in the same folder (owner-only permissions on macOS/Linux); otherwise you sign in
each time the app starts. **File › Sign out** deletes it. The `[cloud]` section lets you point at a
national cloud.

## CSV import and export

**Export list…** in the Action menu saves the current list (users, groups, devices, applications or licences) to a
CSV file, with the columns shown plus each object's ID. It saves what's on screen, so filter and sort
first to export part of a list. The file is UTF-8 with a byte order mark, so Excel shows accented
names correctly. A value Excel would run as a formula (one starting with `=`, `+`, `-` or `@`) is
saved with a leading apostrophe.

**Import users from CSV…** (on Users) creates a user for each row:

| Column | |
| --- | --- |
| Display name | Required |
| User principal name | Required. A name without `@domain` gets the tenant's default domain |
| Password | Optional; generated when blank. At least 8 characters |
| First name, Last name, Job title, Department | Optional |
| Usage location | Optional two-letter country code, needed before licences can be assigned |
| Mail nickname | Optional; taken from the user principal name when blank |
| Enabled, Force change password | Optional `Yes` or `No`; both default to `Yes` |

**Add members from CSV…** (on a selected group) adds the users in a column headed
User principal name, Email or Object ID. A file with one column of user principal names and no
header works too. Users who are already members are left alone.

Headers are matched loosely (`First name`, `firstName` and `given_name` all work) and other columns
are ignored, so a user list exported from here can be edited and imported again. Files saved
by Excel as CSV, CSV UTF-8 or Unicode Text all work, separated by commas, semicolons or tabs.

Before an import changes anything, every row is shown with the problems that will make it be
skipped, such as a missing name, a domain the tenant hasn't verified or a user who already exists.
When it finishes, **Save results…** writes what happened to each row. The results of a user import
include the initial passwords, so keep the file safe and delete it once they've been passed on.

## Exporting to MariaDB

**Export to MariaDB…** in the Action menu saves the current list to a table on a MariaDB server
(MariaDB 10.0.2 or later; MySQL is not supported, as the export uses `ADD COLUMN IF NOT EXISTS`).
Like a CSV export it saves what's on screen, so filter first to export part of a list.

- The table is created if it doesn't exist (`utf8mb4`), with the object's ID (`object_id`, or
  `sku_id` for licences) as the primary key, a `TEXT` column for each column shown, named from its
  header (`Last sign-in` becomes `last_sign_in`), and `exported_at`, the UTC time of the export.
- The table name suggested is `entra_users`, `entra_groups` and so on. An existing table is only
  used if an earlier export made it (it has the ID and `exported_at` columns), so a table that
  belongs to something else is never changed.
- Exporting again updates rows by ID and adds any new rows. Tick **Remove rows for objects not in
  this list** to also delete rows the export didn't include, making the table a copy of the list.
  This is refused while the list is loading, if it failed to load, if it is empty, or if it
  stopped at 20,000 objects, as rows for objects that still exist would be deleted.
- The rows are written in one transaction: if anything fails, the table is left as it was.
- The account needs `CREATE`, `ALTER`, `SELECT`, `INSERT`, `UPDATE` and `DELETE` on the database.

The server, port, user name, database and TLS setting are remembered in the `[mariadb]` section
of `settings.ini`. The password is asked for each time and never saved.

## Logging

Debug information is appended to `output.log` in the same folder as `settings.ini`. It records startup,
sign-in steps (including full AADSTS error responses), every Graph request with its status, duration and
`request-id`, request bodies, the commands you run, and any errors or panics. Passwords, client secrets
and tokens are never written: password and secret fields in request bodies are masked as `***`.
When the file passes 5 MB it is renamed to `output.log.1` at the next start.

Set the detail level in `settings.ini`:

```ini
[logging]
level = debug   ; off, error, warn, info, debug or trace
```

## Keyboard

Everything works without a mouse. Press **F1** in the app for the full list.

| Keys | Action |
| --- | --- |
| F6 / Shift+F6 | Move between console tree, results and Actions pane |
| Tab / Shift+Tab | Move between controls |
| Arrows, PgUp/PgDn, Home/End | Move the selection; ←/→ collapse/expand in the tree; type a letter to jump |
| Enter / Alt+Enter | Properties |
| Shift+F10 | Context menu for the selected item |
| F10 | Menu bar |
| F5 | Refresh |
| Ctrl/⌘+F | Filter the list |
| Ctrl/⌘+N | New user / group |
| Delete | Delete |
| Ctrl/⌘+E, P, L, G, M, R, U | Enable/disable, reset password, assign licence, add to group, add member, assign role, assign licence to user or user/group to application |
| Ctrl+Tab, Insert, Delete, F2 | Property sheets: next tab, add, remove, edit |
| Esc | Close dialog |

Screen readers are supported through AccessKit; list rows announce every column.

## Building

```sh
cargo build --release
./target/release/stormcloud
```

Linux needs the usual windowing development packages for eframe (e.g. `libxkbcommon-dev`,
`libgl1-mesa-dev`, `libwayland-dev` or X11 equivalents).

## Packaging

Pushing a version tag such as `v1.0.0` runs `.github/workflows/release.yml`, which builds the
downloads, tests installing each one, scans them with VirusTotal and publishes them on a GitHub
release with notes from the matching section of `CHANGELOG.md`. The tag must match the version
in `Cargo.toml`, and the repository needs a `VT_API_KEY` secret.

| Download | Built by |
| --- | --- |
| Windows `.msi` (per-user install, no administrator rights needed) | WiX Toolset 6 from `src/installer/stormcloud.wxs` |
| Ubuntu `.deb` (22.04 and later) | `stormcloud --package-deb DIR` (`src/platform/linux.rs`) |
| Apple Silicon `.pkg` (installs into /Applications) | `stormcloud --package-macos DIR` (`src/platform/macos.rs`) |

The two packaging options can also be run by hand from a release build:

```sh
cargo build --release
./target/release/stormcloud --package-deb dist     # Linux, needs dpkg-deb
./target/release/stormcloud --package-macos dist   # macOS, needs the Xcode Command Line Tools
```

The macOS app is signed ad hoc, not with a Developer ID, so Gatekeeper warns the first time it
is opened from a download: right-click it in Applications and choose **Open**.

## Licence

Graphical Cloud Management is free software, released under the
[GNU General Public License, version 3 or later](LICENSE).

The interface uses Google Sans Medium, with Google Sans Bold for headers and titles, under the
SIL Open Font License 1.1 (see `assets/fonts/OFL.txt`). Both are built into the program, so
nothing needs installing.

The app icon is the safe icon by IconMarket, from the
[Casino pack](https://icon-icons.com/pack/casino/4016), licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). It was converted into the image
formats each platform needs (see `assets/icon/CREDITS.txt`).
