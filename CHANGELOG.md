# Changelog

All notable changes to Graphical Cloud Management are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[1.1.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.1.0
[1.0.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.0.0
