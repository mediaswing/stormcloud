# Graphical Cloud Management

A cross-platform desktop console for managing **Microsoft Entra ID**, laid out like a classic
MMC snap-in: console tree on the left, results list in the middle, Actions pane on the right,
with a menu bar, toolbar and status bar. Written in Rust with [egui/eframe](https://github.com/emilk/egui)
and talks to Microsoft Graph directly.

| Node | What you can do |
| --- | --- |
| **Users** | Create (one at a time, in bulk from CSV, or with the **onboarding wizard**), edit, enable/disable, delete, reset password; manage group membership, licences and directory roles; **offboard** leavers; see and manage **authentication methods**, review **sign-ins**, revoke sessions and issue **Temporary Access Passes** |
| **Groups** | Create (security / Microsoft 365, optionally role-assignable), edit, delete; manage members (also in bulk from CSV), owners, licences and **directory role assignments** |
| **Devices** | View details, owners, users and group membership; enable/disable; delete |
| **Applications** | View the enterprise applications connected to the tenant; enable/disable sign-in; delete; set whether assignment is required and whether users see them; manage the users and groups assigned, owners, and review or revoke the permissions granted to them |
| **Exchange Online › Mailboxes** | Create shared mailboxes; convert between user and shared mailboxes; set forwarding and automatic replies; give Full Access, Send As and Send on behalf; hide from address lists; delete shared, room and equipment mailboxes |
| **Exchange Online › Distribution lists** | Create distribution lists and mail-enabled security groups, edit, delete and manage members |
| **Licences** | View subscriptions, service plans and licensed users; assign or remove licences |
| **Remote Servers** | Keep watch over Linux servers by SSH, with a password or an SSH key: operating system and kernel, installed applications, running services, pending (and security) updates and whether a restart is needed. Needs no Microsoft sign-in |

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

Two more delegated Graph permissions are only asked for the first time a feature needs them, so a
tenant that doesn't use those features never grants them:

| Permission | Needed for |
| --- | --- |
| `UserAuthenticationMethod.ReadWrite.All` | The Authentication methods tab, Temporary Access Passes and requiring MFA re-registration |
| `AuditLog.Read.All` | The Sign-ins tab, last sign-in times and MFA registration details |

When one is missing, the app first asks for it without signing in again (which works once an
administrator has granted it), and otherwise offers once a session to have an administrator
approve it. Once granted it is added to **Scopes** in `settings.ini`. To see which Conditional
Access policies applied to a sign-in, also grant `Policy.Read.ConditionalAccess` and add it to
Scopes; without it Microsoft Graph leaves the policies out.

For the Exchange Online node, also add the *delegated* **Office 365 Exchange Online** permission
`Exchange.Manage` (under **APIs my organization uses**) and grant admin consent; see
[Exchange Online](#exchange-online) below.

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
exchange_endpoint = https://outlook.office365.com

[logging]
level = info

[updates]
check = true

[mariadb]
host = db.contoso.local
port = 3306
user = stormcloud
database = entra
tls = true

[interface]
language = en
translation_model =
```

Edit it from **File › Settings…** or by hand. With `remember_sign_in = true` the refresh token is
saved to `token.json` in the same folder (owner-only permissions on macOS/Linux); otherwise you sign in
each time the app starts. **File › Sign out** deletes it. The `[cloud]` section lets you point at a
national cloud.

## Language

**File › Language…** shows the app in English or one of about 30 other languages, including Welsh,
Irish, French, German, Spanish, Polish and Ukrainian. The app is translated on your computer by a
local AI model in [Ollama](https://ollama.com), so its text never leaves it. Install Ollama, get a
model (`ollama pull gemma3:4b` works well), choose a language, then press **Translate the app
into…**. It takes a few minutes; the progress shows in the dialog and the status bar, and **Stop
translating** keeps what has been done so far. Choose a **Translation model** if Ollama has more
than one. Once a language has been translated, choosing it switches the app straight away. The
menu item and the label of the list always say "(Language)" in English too, so you can find your
way back.

Translations by a small AI model can contain mistakes. Each is saved as a JSON file in the
`languages` folder next to `settings.ini`, where you can correct it or share it. Text a translation
leaves out, or gets wrong (for example by losing a `{name}` value), is shown in English, and
translating again fills it in. A file for a language that isn't in the list, such as `eo.json` for
Esperanto, is offered as well, using the `language` name inside it. Only use translation files from
people you trust: a file can't run anything, but it decides what every button and warning says.

Languages written in other alphabets, such as Arabic, Hindi or Chinese, aren't offered yet, because
the window can't draw them. Only the app's own text is translated. What Microsoft sends (error
messages, role and product names) stays as it is, and so does the debug log. CSV exports of a list
and database exports stay in English whatever the language, so the column names and values are the
same each time and an exported user list can be imported again; import results and wizard reports
are in the app's language.

## Exchange Online

The **Exchange Online** node manages mailboxes and distribution lists. Microsoft Graph can't do this,
and Exchange Online has no documented REST API for it, so Graphical Cloud Management uses the same
REST endpoint as the Exchange Online PowerShell module (`/adminapi/beta/{tenant}/InvokeCommand`), which
runs Exchange cmdlets such as `Get-Mailbox` and `Set-Mailbox`. Microsoft could change that endpoint
without notice. Every request is logged with the cmdlet's name, so `output.log` shows what was run.

- It needs the delegated `Exchange.Manage` permission on Office 365 Exchange Online. If that hasn't
  been granted, opening Mailboxes or Distribution lists offers to approve it by signing in again as a
  Global Administrator; your sign-in is kept if that is cancelled.
- Changing things needs an Exchange role, such as Exchange Administrator. Without one, lists may load
  but changes fail with Exchange's own error message.
- Changing a distribution list you don't own uses `-BypassSecurityGroupManagerCheck`, which
  Exchange Administrators are allowed to use.
- Automatic reply times are entered in UTC.
- A user's mailbox can't be deleted here: delete the user under Users, or remove their licence.
- For a national cloud, set `exchange_endpoint` in the `[cloud]` section of `settings.ini`, and
  `exchange_scopes` in `[auth]` if the permission has another name there.

## Authentication and security

A user's properties have three more tabs. **Authentication & security** sums up whether they
can sign in: whether the account is enabled or synced, their last interactive, non-interactive
and successful sign-ins, when the password last changed, whether they are registered for MFA,
their default method, and their methods. Below that, **Troubleshooting** lists what stands
out, such as a disabled account, no MFA method, or the last failed sign-in with what its error
code means (wrong password, smart lockout, blocked by Conditional Access, and so on).

**Authentication methods** lists the Microsoft Authenticator registrations, phone numbers,
passkeys and FIDO2 security keys, Windows Hello for Business, OATH codes, the password reset email
and any Temporary Access Pass. Phone numbers and email addresses are masked (`+44 •••123`), as
the last digits are enough to check with the user. Methods other than the password can be
deleted, with a confirmation that says what stops working.

**Sign-ins** lists the latest 50 interactive sign-ins with the application, result, IP address,
location and Conditional Access result. **Details…** shows everything about one, including the
failure reason and the Conditional Access policies that applied.

The Actions pane also has these for the selected user:

- **Revoke sessions…** invalidates the refresh tokens and browser sessions issued to the user,
  so apps have to sign in again. It can take a few minutes, and access tokens already issued
  stay valid until they expire (usually within an hour), so it doesn't end access straight
  away; disable the account or reset the password to stop them signing in again.
- **Temporary Access Pass…** creates a time-limited pass (10 minutes to 30 days, one use or
  several, as the tenant's policy allows), for example to set up a new phone. It replaces any
  pass the user has, and is shown only once.
- **Require MFA re-registration…** (on the Authentication & security tab) deletes the user's
  MFA methods: Microsoft Authenticator, phones, OATH codes and passkeys. This is how Microsoft
  documents requiring re-registration with Graph. The next time MFA is required they have to
  register again. The password, Windows Hello for Business, platform credentials, the password
  reset email and any Temporary Access Pass are kept. Each deletion's result is shown; a phone
  number that is the default method can be refused.

What these need:

| Feature | Entra role | Licence |
| --- | --- | --- |
| Authentication methods, Temporary Access Pass | Authentication Administrator, or Privileged Authentication Administrator for administrators | — |
| Sign-ins, last sign-in, MFA registration details | Reports Reader, Security Reader, Security Operator, Security Administrator or Global Reader | Microsoft Entra ID P1 or P2 |
| Conditional Access policies on a sign-in | Global Reader, Security Reader, Security Administrator or Conditional Access Administrator | Microsoft Entra ID P1 or P2 |
| Revoke sessions | User Administrator, Helpdesk Administrator (for non-administrators) or Privileged Authentication Administrator | — |

When the role or licence is missing, the tab or action says which is needed. Phone numbers,
sign-in details and Temporary Access Passes are never written to `output.log`.

## Onboarding and offboarding

**Onboard user…** (on Users) is a wizard for a new starter: name, user name and domain, job
title, department, usage location and manager; then licences, showing how many of each are free;
then groups, chosen one by one or copied from a user in a similar role. Copying only ticks groups
whose membership is set here: dynamic groups, groups synced from Active Directory and
distribution lists are listed but can't be ticked, and role-assignable groups are listed
unticked, as their members get directory roles. A random password is generated, and changing it
at first sign-in is required unless you untick it. Before anything is created the review page
lists each step and checks the user name is free.

**Offboard user…** (on a selected user) reads the user's roles, groups, licences and mailbox, and
lets you choose what to do:

1. Block sign-in, and revoke sign-in sessions
2. Remove directory roles, active and eligible (PIM)
3. Convert the mailbox to a shared mailbox, and forward email to someone
4. Remove group memberships (distribution lists and mail-enabled security groups through Exchange Online)
5. Remove licences assigned directly (inherited ones go with the group)
6. Delete the account

They always run in that order. A preview lists every change, which must be ticked as checked
before it runs. Removing a licence, directly or by leaving a group that assigns one, only runs if
the mailbox steps succeeded, as Exchange Online deletes an unlicensed mailbox after 30 days.
Deleting the account only runs if every other step succeeded, and can't be chosen together with
keeping or forwarding the mailbox, because a shared mailbox is deleted with its account. You
can't offboard the account you are signed in with.

Both wizards finish with a report of each step: done, failed (with Microsoft's error) or not run
because a step it needed failed. The workflow is only reported as completed when every step
succeeded. **Save report…** writes it to a CSV file and **Copy report** to the clipboard; the
onboarding report doesn't include the initial password, which is shown separately to copy.

## CSV import and export

**Export list…** in the Action menu saves the current list to a
CSV file, with the columns shown plus each object's ID. It saves what's on screen, so filter and sort
first to export part of a list. Its headers and values are in English whatever the language of the
app. The file is UTF-8 with a byte order mark, so Excel shows accented
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
  English header (`Last sign-in` becomes `last_sign_in`), and `exported_at`, the UTC time of the
  export. Values such as `Yes` and `No` are in English too, whatever the language of the app.
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

## Remote Servers

**Remote Servers** lists servers you check over SSH. It has nothing to do with Microsoft, so it
works without signing in. **Add server…** asks for the host, SSH port, user name and how to sign
in: a **password**, or an **SSH key** file such as `~/.ssh/id_ed25519` (OpenSSH, PEM or PKCS#8;
Ed25519, ECDSA and RSA). **Check now** checks the selected server, and **Refresh** (F5) checks them
all. **Properties** shows what the last check found, on the **General**, **Installed
applications**, **Running services** and **Updates** tabs; Refresh there checks again, and
**Edit…** changes how the server is reached.

- **Passwords and passphrases are never saved.** You are asked the first time a server is
  checked, and the answer is kept in memory until you close the app.
- **Host keys are checked.** A server whose key is in your `~/.ssh/known_hosts` is trusted as it
  is, so servers you already reach with `ssh` need no confirming. For any other server the app
  shows the key's fingerprint to confirm, then saves it in its own `known_hosts` file beside
  `settings.ini`. If a server's key has changed, the app refuses to connect, as `ssh` does.
- **The check only reads.** It runs a short shell script (`REPORT_SCRIPT` in `src/servers.rs`)
  as the user you sign in as, with nothing run through `sudo`, and changes nothing. Package lists
  aren't refreshed, so pending updates are as of the server's own last refresh, and the General
  tab shows when that was (on apt).
- What it reads depends on the package manager: apt (Debian, Ubuntu), dnf or yum (RHEL, Rocky,
  Alma, Fedora), zypper (SUSE), pacman (Arch, listing updates needs `checkupdates`) and apk
  (Alpine). Security updates are counted on apt, dnf, yum and zypper. Whether a restart is
  needed is known on Ubuntu, on Debian with `unattended-upgrades` or `needrestart` installed, on
  dnf and yum when `needs-restarting` is installed, and on zypper; otherwise it is left blank.
  Running services come from systemd, or, without systemd, the list of running programs.
- The list and each server's last results are kept in `servers.json` beside `settings.ini`, so
  they are there when the app starts.

Windows servers aren't supported, as the check needs a Unix shell.

## Updates

At startup Graphical Cloud Management asks GitHub for the latest release, and if it is newer than
the version running, says so. **Download…** opens the release page, where you choose the installer;
nothing is downloaded or installed by the app itself. **Skip this version** stops it mentioning that
version again (it is remembered as `skipped_version` in `settings.ini`), and **Later** asks again
next time.

**Help › Check for updates…** checks at any time, including for a skipped version. The check sends
nothing but an ordinary request to `api.github.com`; turn it off by clearing **Check for updates
when starting** in **File › Settings…**, or with `check = false` in the `[updates]` section.

## Logging

The app keeps a log, `output.log`, in the same folder as `settings.ini`. At the `info` level, the
default, it records startup, sign-in steps (including full AADSTS error responses), the commands you
run, and any errors or panics, with the `request-id` of each failed request. At `debug` it also records
every Graph request with its status, duration and `request-id`, and request bodies, which name users
and groups. Passwords, client secrets and tokens are never written: password and secret fields in
request bodies are masked as `***`. On macOS and Linux only you can read the file.

When the file reaches 5 MB, it is renamed to `output.log.1` (replacing the one before) and a new
one is started, so it never takes more than about 10 MB.

Set the detail level in `settings.ini`:

```ini
[logging]
level = info   ; off, error, warn, info, debug or trace
```

Versions up to 1.5.0 used `debug` by default, and wrote it into `settings.ini`, so an older install
keeps logging at `debug` until you change it.

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
| Ctrl/⌘+N | New user, group, shared mailbox or distribution list |
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
| Windows `.msi` for x86_64 and arm64 (per-user install, no administrator rights needed) | WiX Toolset 6 from `src/installer/stormcloud.wxs` |
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
