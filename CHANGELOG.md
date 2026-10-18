# Changelog

All notable changes to Graphical Cloud Management are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[1.0.0]: https://github.com/mediaswing/stormcloud/releases/tag/v1.0.0
