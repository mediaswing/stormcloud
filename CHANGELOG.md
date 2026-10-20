# Changelog

All notable changes to Graphical Cloud Management are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Language:** the app can be shown in about 30 languages, including Welsh, Irish, French, German,
  Spanish, Polish and Ukrainian, from **File › Language…**. It is translated on your computer by a
  local AI model in Ollama, so nothing is sent over the internet. Each translation is saved as a
  file you can correct or share, and anything not yet translated is shown in English. CSV and
  database exports of a list stay in English, so they read the same whatever the language.

### Changed

- Smaller macOS and Linux downloads: the program no longer includes its symbol table, as on
  Windows.

## [1.3.0] - 2026-10-19

Adds managing users' authentication and security, and wizards for onboarding and offboarding.

### Added

- **Authentication & security:** three new tabs on a user's properties. A summary of whether
  they can sign in, with their last sign-ins, MFA registration and troubleshooting findings; their
  authentication methods (Microsoft Authenticator, phones, passkeys and FIDO2 keys, Windows Hello
  for Business and more, with phone numbers masked), which can be deleted; and their recent
  sign-ins with application, result, IP address, location and Conditional Access results.
- **Revoke sessions**, **Temporary Access Pass** and **Require MFA re-registration** for a user.
- **Onboard user:** a wizard that creates a user with their manager, licences and groups, which
  can be copied from another user, and a generated password that must be changed at first sign-in.
- **Offboard user:** a wizard that blocks sign-in, revokes sessions, removes directory roles
  (including PIM eligibility), converts the mailbox to shared and sets forwarding, removes groups
  and licences, and can delete the account. It shows every change first, runs them in an order
  that keeps the mailbox safe, and skips steps whose prerequisites failed.
- Both wizards end with a report of every step that can be saved or copied, and only report
  success when every step succeeded.
- The permissions `UserAuthenticationMethod.ReadWrite.All` and `AuditLog.Read.All` are asked for
  only when a feature first needs them.

### Fixed

- The saved sign-in (`token.json`) could be readable by other users of the computer for a moment
  while it was written on macOS and Linux, and kept the permissions of an older file. It is now
  created readable only by you.
- New user and Edit now check that a usage location is a two-letter country code, as the CSV
  import does, rather than leaving Microsoft Graph to refuse it.

## [1.2.0] - 2026-10-19

Adds managing connected applications and Exchange Online, and a check for new versions.

### Added

- **Applications:** a new node listing the enterprise applications connected to the tenant (as in
  the Entra admin centre, leaving out Microsoft's own applications and managed identities).
  Enable or disable sign-in, delete, and set whether assignment is required and whether users see
  the application. Its properties show the users and groups assigned (with their app role), its
  owners, and the application and delegated permissions granted to it, which can be revoked.
- New default scopes `Application.ReadWrite.All`, `AppRoleAssignment.ReadWrite.All` and
  `DelegatedPermissionGrant.ReadWrite.All`. After signing in, they are added to a saved Scopes
  setting without them once granted; if they haven't been, you are asked once whether an
  administrator wants to approve them. Sign-in is never refused because of them.
- **Exchange Online:** a new node with **Mailboxes** and **Distribution lists**. Create shared
  mailboxes, convert between user and shared mailboxes, set forwarding and automatic replies,
  manage Full Access, Send As and Send on behalf, and hide mailboxes from address lists. Create,
  edit and delete distribution lists and mail-enabled security groups, and manage their members.
  It uses the REST endpoint behind the Exchange Online PowerShell module, needs the delegated
  `Exchange.Manage` permission (approval is offered if it is missing) and an Exchange role.
- **Update check:** at startup, a dialog says when a newer release is on GitHub, with its notes, a
  link to download it, and a choice to skip that version. **Help › Check for updates…** checks at
  any time, and the startup check can be turned off in Settings.

## [1.1.0] - 2026-10-18

Adds exporting a list to a MariaDB server, and fixes bugs in sign-in and the result list.

### Added

- **Export to MariaDB:** Export to MariaDB… saves the current list to a table on a MariaDB server,
  creating the table if needed and updating rows by object ID on later exports. The connection is
  remembered, apart from the password.

### Fixed

- Turning off "Remember sign-in" now takes effect straight away. Before, the sign-in was saved to
  disk again at the next token refresh, and turning it on did nothing until a restart.
- The selection no longer moves to a different object when the list is refreshed.
- A list still loading when you signed out or changed tenant could appear afterwards, under the
  new sign-in.
- Generated passwords no longer start with `-`, `+` or `=`, which were saved with an apostrophe in
  front in the import results file.
- A temporary error from the Microsoft sign-in service no longer discards the saved sign-in.
- A list that stops at 20,000 objects now says so in the status bar.

## [1.0.0] - 2026-10-18

The first release: a desktop console for Microsoft Entra ID, laid out like a classic MMC snap-in,
that talks to Microsoft Graph directly.

### Added

- **Users:** create, edit, enable or disable, delete and reset passwords; manage group membership,
  licences and directory roles.
- **Groups:** create security and Microsoft 365 groups, optionally role-assignable; edit and delete;
  manage members, owners, licences and directory role assignments.
- **Devices:** view details, owners, users and group membership; enable or disable; delete.
- **Licences:** view subscriptions, service plans and licensed users; assign or remove licences.
- **CSV export:** Export list… saves the current list, as filtered and sorted on screen, in a form
  Excel opens correctly.
- **CSV import:** create users in bulk, or add members to a group, from a CSV file. Every row is
  checked and shown before anything changes, and the results can be saved afterwards.
- Sign-in with a device code in your browser, optionally remembered between sessions, against
  the global or a national Microsoft cloud.
- Full keyboard control (F1 lists the shortcuts) and screen reader support.
- A debug log that records every Graph request with its `request-id`, with passwords, secrets and
  tokens masked.
- Installers: a Windows MSI that installs without administrator rights, an Ubuntu/Debian .deb
  (Ubuntu 22.04 and later), and a macOS .pkg for Apple Silicon (macOS 11 and later).

[1.3.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.3.0
[1.2.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.2.0
[1.1.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.1.0
[1.0.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.0.0
