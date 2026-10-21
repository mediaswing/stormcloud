//! The words the app shows, in English and in any language the user has had translated.
//!
//! Every piece of interface text has a key, such as `menu.file`, and an English version in
//! `ENGLISH`. A translation is a JSON file in the languages folder that gives the text for some
//! or all of the keys in another language. Anything a translation leaves out, or gets wrong, is
//! shown in English instead, so a damaged or out-of-date file never leaves a control without a
//! label.
//!
//! Translations are made by a local AI model in Ollama (see `translate_batch`), so the interface
//! text never leaves the computer. The files are plain JSON, so anyone can correct them, or share
//! them with others.
//!
//! What the app sends to Microsoft, writes to the log or exports to a database stays in English
//! (see `in_english`), so it reads the same whichever language the interface is in.

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

/// The language code for English, which needs no translation file.
pub const ENGLISH_CODE: &str = "en";

/// How many pieces of text are sent to the AI model at once. Small enough for a small model to
/// keep track of, large enough not to be slow.
pub const BATCH_SIZE: usize = 20;

/// Where Ollama listens.
const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Translation files can come from other people, so they are checked before use. The whole
/// catalogue in English is about 60 KB.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Longest language name accepted from a translation file.
const MAX_NAME_CHARS: usize = 60;

/// Every piece of interface text, in British English. `{name}` marks a value filled in when the
/// text is shown; translations must keep these.
const ENGLISH: &[(&str, &str)] = &[
    // Language
    ("language.menu", "Language…"),
    ("language.title", "Language"),
    ("language.label", "Language"),
    ("language.model", "Translation model"),
    ("language.looking", "Looking for models in Ollama…"),
    ("language.no_ollama_short", "Ollama is not running"),
    ("language.no_models_short", "No models installed"),
    (
        "language.no_ollama",
        "Translating needs Ollama, which runs AI models on this computer, so the app's text never leaves it. Install it from ollama.com, start it, and get a model such as gemma3:4b, then look again.\n\n{error}",
    ),
    (
        "language.no_models",
        "Ollama has no models installed. Get one, such as gemma3:4b with \"ollama pull gemma3:4b\", then look again.",
    ),
    ("language.look_again", "Look again"),
    ("language.translate", "Translate the app into {language}"),
    ("language.stop", "Stop translating"),
    (
        "language.note",
        "Translations by a small AI model can contain mistakes. Each is saved in {folder}, where you can correct it or share it. Anything not yet translated is shown in English.",
    ),
    ("language.chosen", "The app is now in {language}."),
    (
        "language.partly",
        "The app is now in {language}, with {count} piece(s) of text still in English. Translate the app again to fill them in.",
    ),
    (
        "language.not_yet",
        "{language} has not been translated yet. Choose Translate the app to translate it.",
    ),
    (
        "language.load_failed",
        "The translation into {language} could not be read, so the app stays as it is: {error}",
    ),
    ("language.translating", "Translating the app into {language} with {model}…"),
    ("language.progress", "Translating the app: {percent}%"),
    ("language.again_title", "Translate again"),
    (
        "language.again_question",
        "The app has already been translated into {language}. Translate all of it again?",
    ),
    ("language.stopped", "Translating stopped. What was translated has been kept."),
    ("language.done", "The app has been translated into {language}."),
    (
        "language.done_partly",
        "The app has been translated into {language}, but {count} piece(s) of text are still in English. Translate the app again to try them again.",
    ),
    ("language.failed", "The app could not be translated: {error}"),
    // Main window
    ("menu.file", "File"),
    ("menu.settings", "Settings…"),
    ("menu.sign_out", "Sign out"),
    ("menu.sign_in", "Sign in"),
    ("menu.exit", "Exit"),
    ("menu.action", "Action"),
    ("menu.no_actions", "No actions available"),
    ("menu.view", "View"),
    ("menu.filter", "Filter…"),
    ("menu.actions_pane", "Actions pane"),
    ("menu.theme", "Theme"),
    ("menu.zoom", "Zoom: Ctrl/⌘ + plus, minus or 0"),
    ("menu.help", "Help"),
    ("menu.shortcuts", "Keyboard shortcuts"),
    ("menu.check_updates", "Check for updates…"),
    ("menu.about", "About Graphical Cloud Management"),
    ("toolbar.filter", "Filter:"),
    ("toolbar.filter_hint", "Type to filter {list}…"),
    ("toolbar.clear_filter", "Clear filter"),
    ("pane.tree", "Console tree"),
    ("pane.tree_item", "{title}, level {level}"),
    ("pane.tree_item_expanded", "{title}, expanded, level {level}"),
    ("pane.tree_item_collapsed", "{title}, collapsed, level {level}"),
    ("pane.actions", "Actions"),
    ("pane.selected_item", "Selected item"),
    ("central.not_signed_in", "You are not signed in."),
    ("central.sign_in", "Sign in…"),
    ("central.retry", "Retry"),
    ("central.empty", "There are no items to show in this view."),
    ("central.no_match", "No items match the filter."),
    ("central.sort", "Sort by this column"),
    // Status bar
    ("status.ready", "Ready"),
    ("status.failed", "{title} failed"),
    ("status.needs_permissions", "{title} needs more permissions"),
    ("status.signing_in", "Signing in…"),
    ("status.requesting_code", "Requesting sign-in code…"),
    ("status.waiting_signin", "Waiting for sign-in in your browser…"),
    ("status.waiting_approval", "Waiting for approval in your browser…"),
    ("status.loading_profile", "Loading profile…"),
    ("status.loading_tenant", "Loading tenant…"),
    ("status.signin_cancelled", "Sign-in cancelled"),
    ("status.permissions_added", "Permissions added"),
    ("status.permissions_not_added", "Permissions not added"),
    ("status.signed_in_as", "Signed in as {account}"),
    ("status.signed_out", "Signed out"),
    ("status.not_signed_in", "Not signed in"),
    ("status.loading_list", "Loading {list}…"),
    ("status.loaded", "{count} {list} loaded"),
    ("status.loaded_truncated", "{count} {list} loaded; the list stops there, so some are missing"),
    ("status.load_failed", "Could not load {list}"),
    ("status.count", "{count} {list}"),
    ("status.shown", "{shown} of {total} shown"),
    ("list.still_loading", "The {list} are still loading."),
    ("list.load_failed", "The {list} did not load. Refresh the list and try again."),
    ("list.truncated", "Only the first {count} {list} were loaded."),
    ("list.not_loaded", "The {list} have not been loaded."),
    ("signin.title", "Sign in"),
    ("tenant.load_title", "Load tenant"),
    // Permissions
    ("scopes.approve_title", "Approve permissions"),
    (
        "scopes.offer_text",
        "{feature} needs Microsoft Graph permissions this app hasn't been granted: {scopes}.\n\nA Global Administrator or Privileged Role Administrator can approve them now by signing in and ticking \"Consent on behalf of your organization\". You stay signed in as you are if that is cancelled. They can also be added on the app registration's API permissions page with admin consent.\n\nSign in to approve them now?",
    ),
    ("scopes.apps_title", "Permissions for Applications"),
    (
        "scopes.apps_text",
        "Changing applications needs more Microsoft Graph permissions than this sign-in has: {scopes}.\n\nA Global Administrator or Privileged Role Administrator can approve them now by signing in and ticking \"Consent on behalf of your organization\". Until then you can view applications but not change them.\n\nSign in to approve them now? You won't be asked again. An administrator can also grant them later on the app registration's API permissions page, and they are then added the next time you sign in.",
    ),
    ("scopes.exchange_title", "Permissions for Exchange"),
    (
        "scopes.exchange_text",
        "Managing Exchange needs the Exchange.Manage permission on Office 365 Exchange Online, which this app hasn't been granted.\n\nA Global Administrator can approve it now by signing in and ticking \"Consent on behalf of your organization\", or add it to the app registration's API permissions and grant admin consent. Changing mailboxes and distribution lists also needs the Exchange Administrator role.\n\nSign in to approve it now?",
    ),
    (
        "scopes.not_added",
        "The permissions for {feature} were not added, and you are still signed in as before.\n\n{error}",
    ),
    // Console tree
    ("node.root", "Microsoft Entra ID"),
    ("node.users", "Users"),
    ("node.groups", "Groups"),
    ("node.devices", "Devices"),
    ("node.applications", "Applications"),
    ("node.exchange", "Exchange Online"),
    ("node.mailboxes", "Mailboxes"),
    ("node.distribution_lists", "Distribution lists"),
    ("node.licences", "Licences"),
    ("node.users_list", "users"),
    ("node.groups_list", "groups"),
    ("node.devices_list", "devices"),
    ("node.applications_list", "applications"),
    ("node.mailboxes_list", "mailboxes"),
    ("node.distribution_lists_list", "distribution lists"),
    ("node.licences_list", "licences"),
    ("node.items_list", "items"),
    (
        "node.users_about",
        "Create, onboard, offboard and edit user accounts; reset passwords; manage authentication methods and review sign-ins",
    ),
    ("node.groups_about", "Manage groups, membership, owners, licences and directory role assignments"),
    ("node.devices_about", "View, enable/disable and delete registered devices"),
    (
        "node.applications_about",
        "Manage connected applications, who can use them and the permissions granted to them",
    ),
    ("node.exchange_about", "Manage Exchange Online mailboxes and distribution lists"),
    (
        "node.mailboxes_about",
        "Create shared mailboxes; set mailbox permissions, forwarding and automatic replies",
    ),
    ("node.distribution_lists_about", "Create and delete distribution lists and manage their members"),
    ("node.licences_about", "View subscriptions and assign or remove licences"),
    ("node.remote_servers", "Remote Servers"),
    ("node.remote_servers_list", "servers"),
    (
        "node.remote_servers_about",
        "Keep watch over Linux servers by SSH: operating system, installed and running software, and pending updates",
    ),
    // Column titles. Database exports name their columns after the English.
    ("col.display_name", "Display name"),
    ("col.upn", "User principal name"),
    ("col.enabled", "Enabled"),
    ("col.type", "Type"),
    ("col.job_title", "Job title"),
    ("col.department", "Department"),
    ("col.licences", "Licences"),
    ("col.membership", "Membership"),
    ("col.role_assignable", "Role-assignable"),
    ("col.email", "Email"),
    ("col.description", "Description"),
    ("col.name", "Name"),
    ("col.os", "OS"),
    ("col.version", "Version"),
    ("col.join_type", "Join type"),
    ("col.compliant", "Compliant"),
    ("col.managed", "Managed"),
    ("col.last_sign_in", "Last sign-in"),
    ("col.publisher", "Publisher"),
    ("col.assignment_required", "Assignment required"),
    ("col.visible_to_users", "Visible to users"),
    ("col.application_id", "Application ID"),
    ("col.product", "Product"),
    ("col.sku", "SKU"),
    ("col.assigned", "Assigned"),
    ("col.total", "Total"),
    ("col.available", "Available"),
    ("col.status", "Status"),
    ("col.applies_to", "Applies to"),
    ("col.email_address", "Email address"),
    ("col.forwarding_to", "Forwarding to"),
    ("col.hidden", "Hidden"),
    ("col.internal_senders_only", "Internal senders only"),
    ("col.host", "Host"),
    ("col.kernel", "Kernel"),
    ("col.installed_apps", "Installed"),
    ("col.running_services", "Running"),
    ("col.pending_updates", "Pending updates"),
    ("col.reboot_required", "Restart needed"),
    ("col.last_checked", "Last checked"),
    ("col.installed_version", "Installed version"),
    ("col.available_version", "Available version"),
    ("col.security", "Security"),
    // Values
    ("value.yes", "Yes"),
    ("value.no", "No"),
    ("value.group_m365", "Microsoft 365"),
    ("value.group_mail_security", "Mail-enabled security"),
    ("value.group_security", "Security"),
    ("value.group_distribution", "Distribution"),
    ("value.group_other", "Other"),
    ("value.dynamic", "Dynamic"),
    ("value.assigned", "Assigned"),
    ("value.entra_joined", "Entra joined"),
    ("value.hybrid_joined", "Hybrid joined"),
    ("value.entra_registered", "Entra registered"),
    // Dialogs
    ("dialog.ok", "OK"),
    ("dialog.yes", "Yes"),
    ("dialog.no", "No"),
    ("dialog.cancel", "Cancel"),
    ("dialog.close", "Close"),
    ("dialog.copy", "Copy to clipboard"),
    ("status.copied", "Copied to clipboard"),
    ("status.searching", "Searching…"),
    ("form.required", "{field} is required."),
    ("picker.hint", "Type to search"),
    ("picker.no_matches", "No matches."),
    ("picker.keys", "↑/↓ to choose, Enter to select, Esc to cancel"),
    ("picker.select", "Select"),
    ("signin.heading", "Sign in to Microsoft Entra ID"),
    ("signin.code", "Code:"),
    ("signin.copy_and_open", "Copy code and open browser"),
    ("signin.copy", "Copy code"),
    ("update.title", "Update available"),
    ("update.available", "Graphical Cloud Management {version} is available. You have version {current}."),
    ("update.whats_new", "What's new"),
    ("update.download", "Download…"),
    ("update.download_tip", "Open the release page in your browser"),
    ("update.skip", "Skip this version"),
    ("update.later", "Later"),
    ("about.version", "Version {version}"),
    ("about.summary", "A console for managing Microsoft Entra ID through Microsoft Graph."),
    ("about.licence", "Free software under the GNU General Public License, version 3 or later."),
    ("about.settings", "Settings: {path}"),
    // Keyboard shortcuts
    ("help.panes", "Move between the console tree, results and Actions pane"),
    ("help.controls", "Move between controls"),
    ("help.selection", "Move the selection"),
    ("help.expand", "Collapse / expand in the console tree"),
    ("help.properties", "Open the selected item's properties"),
    ("help.context_menu", "Actions for the selected item (context menu)"),
    ("help.menu_bar", "Move to the menu bar"),
    ("help.filter", "Filter the results list"),
    ("help.new", "New user, group, shared mailbox or distribution list"),
    ("help.delete", "Delete the selected item"),
    ("help.enable", "Enable or disable the selected item"),
    ("help.password", "Reset a user's password"),
    ("help.licence", "Assign a licence"),
    ("help.group", "Add to group / add member"),
    ("help.role", "Assign a directory role"),
    ("help.assign", "Assign the selected licence to a user, or a user or group to the selected application"),
    ("help.tabs", "Next / previous tab in a properties sheet"),
    ("help.tab_ops", "Add, remove, edit on a properties tab"),
    ("help.settings", "Settings"),
    ("help.zoom", "Zoom in / out / reset"),
    ("help.close", "Close the dialog"),
    // Actions
    ("act.open", "Open"),
    ("act.refresh", "Refresh"),
    ("act.new", "New…"),
    ("act.new_user", "New user…"),
    ("act.new_group", "New group…"),
    ("act.new_shared_mailbox", "New shared mailbox…"),
    ("act.new_distribution_list", "New distribution list…"),
    ("act.properties", "Properties"),
    ("act.disable_account", "Disable account"),
    ("act.enable_account", "Enable account"),
    ("act.disable_sign_in", "Disable sign-in"),
    ("act.enable_sign_in", "Enable sign-in"),
    ("act.disable", "Disable"),
    ("act.enable", "Enable"),
    ("act.reset_password", "Reset password…"),
    ("act.assign_licence", "Assign licence…"),
    ("act.add_to_group", "Add to group…"),
    ("act.add_member", "Add member…"),
    ("act.assign_role", "Assign directory role…"),
    ("act.assign_users_groups", "Assign users and groups…"),
    ("act.assign_to_user", "Assign to user…"),
    ("act.delete", "Delete"),
    ("act.export_list", "Export list…"),
    ("act.export_database", "Export to MariaDB…"),
    ("act.import_users", "Import users from CSV…"),
    ("act.import_members", "Add members from CSV…"),
    ("act.forwarding", "Forwarding…"),
    ("act.auto_replies", "Automatic replies…"),
    ("act.convert_to_user", "Convert to user mailbox"),
    ("act.convert_to_shared", "Convert to shared mailbox"),
    ("act.onboard", "Onboard user…"),
    ("act.offboard", "Offboard user…"),
    ("act.security", "Authentication & security…"),
    ("act.revoke_sessions", "Revoke sessions…"),
    ("act.temporary_access_pass", "Temporary Access Pass…"),
    ("act.add_server", "Add server…"),
    ("act.check_server", "Check now"),
    ("act.remove_server", "Remove from list"),
    // Tasks, named in the status bar while they run and in the title of an error if they fail
    ("status.working", "{task}…"),
    ("status.done", "{task}: done"),
    ("task.enable", "Enable {name}"),
    ("task.disable", "Disable {name}"),
    ("task.delete", "Delete {name}"),
    ("task.add_member", "Add member"),
    ("task.add_owner", "Add owner"),
    ("task.assign_role", "Assign directory role"),
    ("task.assign_licence", "Assign licence"),
    ("task.remove_licence", "Remove licence"),
    ("task.assign_app", "Assign to application"),
    ("task.save_changes", "Save changes"),
    // Pickers
    ("pick.assign_licence_to", "Assign licence to {name}"),
    ("pick.add_to_group", "Add {name} to group"),
    ("pick.add_member_to", "Add member to {name}"),
    ("pick.assign_role_to", "Assign directory role to {name}"),
    ("pick.assign_to_user", "Assign {name} to user"),
    ("pick.assign_user_or_group_to", "Assign user or group to {name}"),
    ("pick.add_owner_to", "Add owner to {name}"),
    ("pick.role_for", "Role for {name}"),
    ("pick.name_or_upn", "Name or UPN:"),
    ("pick.group_name", "Group name:"),
    ("pick.search", "Search:"),
    ("pick.role", "Role:"),
    ("pick.licence", "Licence:"),
    ("pick.sku_detail", "{sku} · {available} of {total} available"),
    // Settings
    ("settings.title", "Settings"),
    (
        "settings.intro",
        "Enter the directory (tenant) ID and the application (client) ID of an app registration with \"Allow public client flows\" enabled and delegated Microsoft Graph permissions.",
    ),
    ("settings.tenant", "Tenant ID"),
    ("settings.client", "Client ID"),
    ("settings.remember", "Remember sign-in on this computer"),
    ("settings.scopes", "Scopes"),
    ("settings.updates", "Check for updates when starting"),
    ("settings.stored_in", "Stored in"),
    ("settings.bad_client", "Client ID must be a GUID, for example 00000000-0000-0000-0000-000000000000."),
    ("settings.bad_tenant", "Tenant ID must be a GUID or a domain such as contoso.onmicrosoft.com."),
    // Form buttons and fields
    ("dialog.save", "Save"),
    ("dialog.create", "Create"),
    ("field.display_name", "Display name"),
    ("field.user_name", "User name"),
    ("field.domain", "Domain"),
    ("field.first_name", "First name"),
    ("field.last_name", "Last name"),
    ("field.job_title", "Job title"),
    ("field.department", "Department"),
    ("field.usage_location", "Usage location"),
    ("field.password", "Password"),
    ("field.new_password", "New password"),
    ("field.force_change", "Require password change at next sign-in"),
    ("field.account_enabled", "Account enabled"),
    ("field.group_type", "Group type"),
    ("field.name", "Name"),
    ("field.mail_nickname", "Mail nickname"),
    ("field.description", "Description"),
    ("field.role_assignable", "Microsoft Entra roles can be assigned to this group"),
    // Users and groups
    ("user.new_title", "New user"),
    (
        "user.new_intro",
        "Usage location is a two-letter country code (e.g. GB) and is required before licences can be assigned.",
    ),
    ("user.bad_alias", "User name must not contain spaces or @; choose the domain separately."),
    ("user.short_password", "Password must be at least 8 characters."),
    ("user.bad_usage_location", "Usage location must be a two-letter country code, such as GB."),
    ("user.creating", "Creating user…"),
    ("user.created_title", "User created"),
    ("user.created", "{upn} was created. Give the user this initial password:"),
    ("password.title", "Reset password — {name}"),
    ("password.reset", "Reset"),
    ("password.resetting", "Resetting password…"),
    ("password.done_title", "Password reset"),
    ("password.done", "The password for {name} was reset to:"),
    ("password.reset_title", "Reset password"),
    ("group.new_title", "New group"),
    (
        "group.new_intro",
        "Role-assignable groups can only be created by a Privileged Role Administrator or Global Administrator, and the setting cannot be changed later.",
    ),
    ("group.bad_nickname", "Enter a mail nickname using letters and numbers."),
    ("group.creating", "Creating group…"),
    ("group.created", "Created group {name}"),
    (
        "ops.role_assignable_only",
        "Directory roles can only be assigned to groups created as role-assignable (isAssignableToRole). This can only be set when a group is created.",
    ),
    ("ops.assign_app_title", "Assign users and groups"),
    (
        "ops.no_user_roles",
        "{name} has app roles, but none that can be given to users or groups, so they can't be assigned to it.",
    ),
    ("ops.disable_user_title", "Disable user"),
    ("ops.disable_user", "Disable user '{name}'? The user will be unable to sign in."),
    ("ops.disable_application_title", "Disable application"),
    (
        "ops.disable_application",
        "Disable application '{name}'? No one will be able to sign in to it, and it will be unable to get tokens.",
    ),
    ("ops.disable_device_title", "Disable device"),
    (
        "ops.disable_device",
        "Disable device '{name}'? The device will be unable to authenticate with Microsoft Entra ID.",
    ),
    ("ops.delete_user_title", "Delete user"),
    (
        "ops.delete_user",
        "Are you sure you want to delete the user '{name}'? Deleted users can be restored for 30 days.",
    ),
    ("ops.delete_group_title", "Delete group"),
    (
        "ops.delete_group",
        "Are you sure you want to delete the group '{name}'? Microsoft 365 groups can be restored for 30 days; security groups are deleted permanently.",
    ),
    ("ops.delete_device_title", "Delete device"),
    ("ops.delete_device", "Are you sure you want to delete the device '{name}'? This cannot be undone."),
    ("ops.delete_application_title", "Delete application"),
    (
        "ops.delete_application",
        "Are you sure you want to delete the application '{name}'? It is removed from the tenant with its user and group assignments and the permissions granted to it, so no one will be able to sign in to it.",
    ),
    // Property sheets
    ("tab.general", "General"),
    ("tab.member_of", "Member of"),
    ("tab.licences", "Licences"),
    ("tab.roles", "Directory roles"),
    ("tab.members", "Members"),
    ("tab.owners", "Owners"),
    ("tab.registered_owners", "Registered owners"),
    ("tab.registered_users", "Registered users"),
    ("tab.service_plans", "Service plans"),
    ("tab.licensed_users", "Licensed users"),
    ("tab.app_users", "Users and groups"),
    ("tab.permissions", "Permissions"),
    ("tab.mailbox_access", "Mailbox permissions"),
    ("tab.auto_replies", "Automatic replies"),
    ("tab.security", "Authentication & security"),
    ("tab.auth_methods", "Authentication methods"),
    ("tab.sign_ins", "Sign-ins"),
    ("tab.installed_apps", "Installed applications"),
    ("tab.running_services", "Running services"),
    ("tab.updates", "Updates"),
    ("status.loading_tab", "Loading the {tab} tab…"),
    ("sheet.title", "{name} Properties"),
    ("sheet.tab", "{tab} tab"),
    ("sheet.ctrl_tab", "Ctrl+Tab switches tabs"),
    ("sheet.loading", "Loading…"),
    ("sheet.empty", "There are no items to show."),
    ("sheet.edit", "Edit…"),
    ("sheet.edit_title", "Edit {name}"),
    ("sheet.remove", "Remove"),
    ("sheet.remove_from_group", "Remove from group"),
    ("sheet.assign_role", "Assign role…"),
    ("sheet.remove_assignment", "Remove assignment"),
    ("sheet.remove_member", "Remove member"),
    ("sheet.add_owner", "Add owner…"),
    ("sheet.remove_owner", "Remove owner"),
    ("sheet.assign_user_or_group", "Assign user or group…"),
    ("sheet.revoke_permission", "Revoke permission"),
    ("sheet.add_permission", "Add permission…"),
    ("sheet.remove_permission", "Remove permission"),
    ("sheet.remove_from_user", "Remove from user"),
    ("sheet.role_assignable_only", "Only role-assignable groups can be assigned directory roles."),
    ("sheet.remove_from_question", "Remove {name} from '{from}'?"),
    ("sheet.remove_member_question", "Remove '{member}' from the members of {name}?"),
    ("sheet.remove_owner_question", "Remove '{member}' from the owners of {name}?"),
    ("sheet.remove_role_question", "Remove the role '{role}' from {name}?"),
    ("sheet.remove_licence_question", "Remove this licence from {name}?"),
    ("sheet.remove_assignment_question", "Remove '{member}' from {name}?"),
    (
        "sheet.revoke_application_question",
        "Revoke the application permission {permission} on {api} from {name}? It will no longer be able to use it without signing in as a user.",
    ),
    (
        "sheet.revoke_delegated_question",
        "Revoke the delegated permissions on {api} granted to {name} for {user}? This revokes all of them: {permissions}. Users may be asked to consent again the next time they sign in.",
    ),
    (
        "sheet.revoke_delegated_all_question",
        "Revoke the delegated permissions on {api} granted to {name} for all users? This revokes all of them: {permissions}. Users may be asked to consent again the next time they sign in.",
    ),
    (
        "sheet.app_intro",
        "When assignment is required, only the users and groups on the Users and groups tab can sign in. Applications that are not visible are left out of My Apps and the Microsoft 365 app launcher.",
    ),
    ("field.company", "Company"),
    ("field.office", "Office"),
    ("field.mobile_phone", "Mobile phone"),
    ("field.employee_id", "Employee ID"),
    ("col.service_plans", "Service plans"),
    ("col.sku_id", "SKU ID"),
    ("col.disabled_plans", "Disabled plans"),
    ("col.role", "Role"),
    ("col.scope", "Scope"),
    ("col.upn_or_email", "UPN / email"),
    ("col.service_plan", "Service plan"),
    ("col.api", "API"),
    ("col.permissions", "Permissions"),
    ("col.granted_for", "Granted for"),
    ("value.user", "User"),
    ("value.group", "Group"),
    ("value.device", "Device"),
    ("value.directory_role", "Directory role"),
    ("value.contact", "Contact"),
    ("value.application", "Application"),
    ("value.default_access", "Default access"),
    ("value.admin_consent", "Admin consent"),
    ("value.all_users_admin_consent", "All users (admin consent)"),
    ("value.delegated", "Delegated"),
    // Properties
    ("prop.product", "Product"),
    ("prop.sku_part", "SKU part number"),
    ("prop.sku_id", "SKU ID"),
    ("prop.status", "Status"),
    ("prop.applies_to", "Applies to"),
    ("prop.assigned", "Assigned"),
    ("prop.total_enabled", "Total (enabled)"),
    ("prop.available", "Available"),
    ("prop.suspended", "Suspended"),
    ("prop.in_warning", "In warning"),
    ("prop.name", "Name"),
    ("prop.display_name", "Display name"),
    ("prop.first_name", "First name"),
    ("prop.last_name", "Last name"),
    ("prop.upn", "User principal name"),
    ("prop.email", "Email"),
    ("prop.job_title", "Job title"),
    ("prop.department", "Department"),
    ("prop.company", "Company"),
    ("prop.office", "Office"),
    ("prop.mobile_phone", "Mobile phone"),
    ("prop.business_phones", "Business phones"),
    ("prop.usage_location", "Usage location"),
    ("prop.employee_id", "Employee ID"),
    ("prop.account_enabled", "Account enabled"),
    ("prop.user_type", "User type"),
    ("prop.synced", "Synced from on-premises"),
    ("prop.created", "Created"),
    ("prop.password_changed", "Password last changed"),
    ("prop.object_id", "Object ID"),
    ("prop.publisher", "Publisher"),
    ("prop.sign_in_enabled", "Enabled for users to sign in"),
    ("prop.assignment_required", "Assignment required"),
    ("prop.homepage", "Homepage"),
    ("prop.sign_on_url", "Sign-on URL"),
    ("prop.reply_urls", "Reply URLs"),
    ("prop.sso_mode", "Single sign-on mode"),
    ("prop.audience", "Sign-in audience"),
    ("prop.notes", "Notes"),
    ("prop.application_id", "Application ID"),
    ("prop.visible_to_users", "Visible to users"),
    ("prop.description", "Description"),
    ("prop.mail_nickname", "Mail nickname"),
    ("prop.security_enabled", "Security enabled"),
    ("prop.mail_enabled", "Mail enabled"),
    ("prop.group_types", "Group types"),
    ("prop.role_assignable", "Role-assignable"),
    ("prop.membership_rule", "Membership rule"),
    ("prop.rule_processing", "Rule processing"),
    ("prop.visibility", "Visibility"),
    ("prop.os", "Operating system"),
    ("prop.os_version", "OS version"),
    ("prop.host", "Host"),
    ("prop.port", "Port"),
    ("prop.ssh_user", "Signs in as"),
    ("prop.sign_in_method", "Signs in with"),
    ("prop.last_checked", "Last checked"),
    ("prop.host_name", "Host name"),
    ("prop.kernel", "Kernel"),
    ("prop.architecture", "Architecture"),
    ("prop.uptime", "Up for"),
    ("prop.package_manager", "Package manager"),
    ("prop.installed_packages", "Installed packages"),
    ("prop.running_services", "Running services"),
    ("prop.running_processes", "Running programs"),
    ("prop.pending_updates", "Pending updates"),
    ("prop.reboot_required", "Restart needed"),
    ("prop.lists_updated", "Package lists last refreshed"),
    ("prop.packages_changed", "Packages last changed"),
    ("prop.join_type", "Join type"),
    ("prop.manufacturer", "Manufacturer"),
    ("prop.model", "Model"),
    ("prop.enabled", "Enabled"),
    ("prop.compliant", "Compliant"),
    ("prop.managed", "Managed"),
    ("prop.profile_type", "Profile type"),
    ("prop.registered", "Registered"),
    ("prop.last_sign_in", "Last sign-in"),
    ("prop.device_id", "Device ID"),
    // Export to MariaDB
    ("db.title", "Export to MariaDB"),
    ("db.export", "Export"),
    (
        "db.intro",
        "Saves the list as shown to a table, created if it does not exist. Exporting again updates rows by object ID. The password is not saved. Column names and values are in English, whatever the language of the app.",
    ),
    ("db.server", "Server"),
    ("db.port", "Port"),
    ("db.database", "Database"),
    ("db.table", "Table"),
    ("db.tls", "Encrypt the connection (TLS)"),
    ("db.replace", "Remove rows for objects not in this list"),
    ("db.bad_port", "Port must be a number from 1 to 65535."),
    ("db.bad_table", "Table may only contain letters, digits and underscores."),
    ("db.incomplete", "{why} Untick \"Remove rows…\" to export what is shown."),
    ("db.empty", "The list is empty, so every row would be removed. Untick \"Remove rows…\" to export."),
    ("db.exporting", "Exporting {list} to {database}.{table} on {host}…"),
    ("db.exported", "Exported {count} {list} to {database}.{table} on {host}"),
    (
        "db.not_ours",
        "The table {table} already exists and was not made by this export of the list: it has no `{column}` and `exported_at` columns. Choose another table name.",
    ),
    (
        "db.not_ours_licences",
        "The table {table} already exists and was not made by this export of the licences: it has no `{column}` and `exported_at` columns. Choose another table name.",
    ),
    ("db.connect_failed", "could not connect to {host}:{port}"),
    ("db.lookup_failed", "could not look up the table {table}"),
    ("db.prepare_failed", "could not prepare the table {table}"),
    ("db.write_failed", "could not write to the table {table}"),
    ("db.remove_failed", "could not remove old rows from {table}"),
    // CSV files
    ("csv.file_type", "CSV file"),
    ("csv.export_title", "Export list"),
    ("csv.exported", "Exported {count} {list} to {path}"),
    ("csv.save_failed", "could not write {path}"),
    ("csv.read_failed", "Could not read {path}: {error}"),
    ("csv.write_failed", "could not write the CSV: {error}"),
    ("csv.invalid", "The file is not valid CSV: {error}"),
    ("csv.empty", "The file is empty."),
    (
        "csv.user_format",
        "Columns, headed in English: \"Display name\" and \"User principal name\" (required); \"Password\", \"First name\", \"Last name\", \"Job title\", \"Department\", \"Usage location\", \"Mail nickname\", \"Enabled\" and \"Force change password\" (optional). A blank password is generated; Enabled and Force change password default to Yes.",
    ),
    (
        "csv.member_format",
        "One column headed \"User principal name\", \"Email\" or \"Object ID\", or a single column of user principal names with no header.",
    ),
    ("csv.no_column", "The file has no {column} column.\n\n{format}"),
    ("csv.no_user_column", "The file has no column of users.\n\n{format}"),
    ("csv.no_users", "The file has a header line but no users.\n\n{format}"),
    ("csv.no_members", "The file has no users in it.\n\n{format}"),
    ("csv.no_display_name", "Display name is missing"),
    ("csv.no_upn", "User principal name is missing"),
    ("csv.bad_upn", "'{upn}' is not a valid user principal name"),
    ("csv.unverified_domain", "{domain} is not a verified domain of this tenant"),
    ("csv.user_exists", "A user with this name already exists"),
    ("csv.same_upn", "Same user principal name as line {line}"),
    ("csv.short_password", "Password must be at least 8 characters"),
    ("csv.bad_usage_location", "Usage location '{usage}' is not a two-letter country code"),
    ("csv.no_nickname", "Mail nickname is missing"),
    ("csv.bad_enabled", "Enabled must be Yes or No"),
    ("csv.bad_force_change", "Force change password must be Yes or No"),
    ("csv.same_user", "Same user as line {line}"),
    ("csv.not_a_user", "Not a user principal name, email address or object ID"),
    // Imports
    ("bulk.users_title", "Import users from CSV"),
    (
        "bulk.users_intro",
        "Check the users below. Rows with a problem are skipped. A user name without a domain gets the tenant's default domain. The initial passwords are in the results, which you can save once the users are created.",
    ),
    ("bulk.ignored", "Ignored columns: {columns}."),
    ("bulk.generated", "Generated"),
    ("bulk.from_file", "From file"),
    ("bulk.ready", "Ready"),
    ("bulk.creating_users", "Creating {count} users…"),
    ("bulk.created", "Created"),
    ("bulk.created_users", "Created {count} users"),
    ("bulk.created_users_failed", "Created {count} users; {failed} failed"),
    ("bulk.create_user", "Create 1 user"),
    ("bulk.create_users", "Create {count} users"),
    ("bulk.new_users_file", "new users"),
    (
        "bulk.passwords_note",
        "The results file holds the initial passwords. Keep it somewhere safe and delete it once they have been passed on.",
    ),
    ("bulk.members_title", "Add members to {name} from CSV"),
    (
        "bulk.dynamic_group",
        "This group has dynamic membership, so its members are set by its rule and can't be added.",
    ),
    (
        "bulk.members_intro",
        "Check the users below. Rows with a problem are skipped, and users who are already members are left as they are.",
    ),
    ("bulk.adding_members", "Adding {count} members…"),
    ("bulk.added", "Added"),
    ("bulk.already_member", "Already a member"),
    ("bulk.added_members", "Added {count} members"),
    ("bulk.added_members_failed", "Added {count} members; {failed} failed"),
    ("bulk.add_member", "Add 1 member"),
    ("bulk.add_members", "Add {count} members"),
    ("bulk.added_members_file", "added members"),
    ("bulk.preview_summary", "{ready} ready, {skip} to skip"),
    ("bulk.running_summary", "Working on {count} rows…"),
    ("bulk.done_summary", "{done} done, {failed} failed, {skipped} skipped"),
    ("bulk.working", "Working… this can take a while for a long file."),
    ("bulk.save_results", "Save results…"),
    ("bulk.save_results_title", "Save results"),
    ("bulk.saved_results", "Saved results to {path}"),
    ("bulk.several_users", "More than one user has this email address"),
    ("bulk.no_user", "No user has this user principal name or email address"),
    ("col.line", "Line"),
    ("col.password", "Password"),
    ("col.initial_password", "Initial password"),
    ("col.result", "Result"),
    ("col.user", "User"),
    // Exchange Online
    ("exchange.no_initial_domain", "Could not find the tenant's onmicrosoft.com domain."),
    ("exchange.not_found", "{name} was not found."),
    ("exchange.full_access", "Full Access"),
    ("exchange.send_as", "Send As"),
    ("exchange.send_on_behalf", "Send on behalf"),
    ("exchange.full_access_about", "Open the mailbox and read, send and delete its email"),
    ("exchange.send_as_about", "Send email that appears to come from the mailbox"),
    ("exchange.send_on_behalf_about", "Send email shown as sent on behalf of the mailbox"),
    ("exchange.new_mailbox_title", "New shared mailbox"),
    (
        "exchange.new_mailbox_intro",
        "A shared mailbox doesn't need a licence while it is under 50 GB. Give people access to it on its Mailbox permissions tab.",
    ),
    ("exchange.creating_mailbox", "Creating shared mailbox…"),
    ("exchange.created_mailbox", "Created shared mailbox {address}"),
    ("exchange.new_list_title", "New distribution list"),
    ("exchange.creating_list", "Creating distribution list…"),
    ("exchange.created_list", "Created distribution list {address}"),
    (
        "exchange.forward_intro",
        "Leave \"Forward email to\" blank to stop forwarding. Forwarding to an address outside the organisation only works if the outbound spam policy allows it.",
    ),
    ("exchange.bad_forward", "Enter the email address to forward to, such as ann@contoso.com."),
    ("exchange.bad_alias", "Email name must not contain spaces or @; choose the domain separately."),
    ("exchange.convert_title", "Convert mailbox"),
    ("exchange.convert_only", "Only user and shared mailboxes can be converted."),
    (
        "exchange.convert_to_shared",
        "Convert {name} to a shared mailbox? Its account can no longer sign in, and people with Full Access open it from their own mailbox. Under 50 GB it doesn't need a licence, so you can remove the user's licence afterwards.",
    ),
    (
        "exchange.convert_to_user",
        "Convert {name} to a user mailbox? Its user needs a licence that includes Exchange Online within 30 days, and a password and an enabled account to sign in.",
    ),
    ("exchange.delete_mailbox_title", "Delete mailbox"),
    ("exchange.delete_list_title", "Delete distribution list"),
    (
        "exchange.delete_user_mailbox",
        "A user's mailbox goes with the user: delete the user under Users, or remove their Exchange Online licence. To keep the email, convert it to a shared mailbox first.",
    ),
    (
        "exchange.delete_shared_mailbox",
        "Delete the shared mailbox '{name}' and its email? It can be restored for 30 days.",
    ),
    (
        "exchange.delete_room_mailbox",
        "Delete the room mailbox '{name}' and its email? It can be restored for 30 days.",
    ),
    (
        "exchange.delete_equipment_mailbox",
        "Delete the equipment mailbox '{name}' and its email? It can be restored for 30 days.",
    ),
    (
        "exchange.delete_mailbox",
        "Delete the mailbox '{name}' and its email? It can be restored for 30 days.",
    ),
    ("exchange.delete_list", "Delete the distribution list '{name}'? This cannot be undone."),
    ("exchange.no_address", "{name} has no email address, so it can't be added to a distribution list."),
    ("exchange.give_access", "Give a user access to {name}"),
    ("exchange.permission_for", "Permission for {name}"),
    ("exchange.permission", "Permission:"),
    ("exchange.remove_permission", "Remove the {permission} permission of {user} on {name}?"),
    ("task.save_mailbox", "Save mailbox"),
    ("task.save_replies", "Save automatic replies"),
    ("task.save_list", "Save distribution list"),
    ("task.convert", "Convert {name}"),
    ("task.give", "Give {permission}"),
    ("replies.title", "Automatic replies — {name}"),
    ("replies.intro", "Times are in UTC, written as 2026-12-24 17:00."),
    ("replies.on", "On"),
    ("replies.off", "Off"),
    ("replies.scheduled", "On, between these times"),
    ("replies.scheduled_below", "On, between the times below"),
    ("replies.start", "Start"),
    ("replies.end", "End"),
    ("replies.inside", "Reply inside the organisation"),
    ("replies.outside", "Reply outside the organisation"),
    ("replies.audience", "Outside the organisation"),
    ("replies.everyone", "Reply to everyone"),
    ("replies.known", "Reply only to senders in the mailbox's contacts"),
    ("replies.known_only", "Only to senders in the mailbox's contacts: {message}"),
    ("replies.dont_reply", "Don't reply"),
    ("replies.bad_times", "Enter the start and end as a date and time, such as 2026-12-24 17:00."),
    ("replies.end_before_start", "The end must be after the start."),
    ("value.mailbox_user", "User"),
    ("value.mailbox_shared", "Shared"),
    ("value.mailbox_room", "Room"),
    ("value.mailbox_equipment", "Equipment"),
    ("value.distribution_list", "Distribution list"),
    ("value.mail_security_group", "Mail-enabled security group"),
    ("value.room_list", "Room list"),
    ("col.permission", "Permission"),
    ("field.email_name", "Email name"),
    ("field.type", "Type"),
    ("field.hide", "Hide from address lists"),
    ("field.forward_to", "Forward email to"),
    ("field.keep_copy", "Keep a copy in the mailbox when forwarding"),
    ("prop.email_address", "Email address"),
    ("prop.type", "Type"),
    ("prop.email_addresses", "Email addresses"),
    ("prop.alias", "Alias"),
    ("prop.hidden", "Hidden from address lists"),
    ("prop.forwarding_to", "Forwarding to"),
    ("prop.keep_copy", "Keep a copy when forwarding"),
    ("prop.archive", "Archive"),
    ("prop.litigation_hold", "Litigation hold"),
    ("prop.internal_only", "Only people in the organisation can send to it"),
    ("prop.owners", "Owners"),
    // Authentication and security
    (
        "security.needs_premium",
        "Sign-in logs, sign-in activity and MFA registration details need a Microsoft Entra ID P1 or P2 licence in the tenant.\n\nMicrosoft Graph said: {error}",
    ),
    (
        "security.roles_reports",
        "Reading sign-in logs and registration details needs one of these roles: Reports Reader, Security Reader, Security Operator, Security Administrator or Global Reader.",
    ),
    (
        "security.roles_methods",
        "Managing another user's authentication methods needs the Authentication Administrator role, or Privileged Authentication Administrator when the user is an administrator. When you change your own methods, Microsoft also wants an MFA sign-in in the last 10 minutes: sign out and in again, then retry.",
    ),
    (
        "security.roles_users",
        "This needs a role that can manage the user, such as User Administrator, Helpdesk Administrator (for non-administrators) or Privileged Authentication Administrator.",
    ),
    (
        "security.not_allowed",
        "Your account isn't allowed to do this. {roles}\n\nMicrosoft Graph said: {error}",
    ),
    ("security.unavailable_scope", "Not available: needs the {scopes} permission"),
    ("security.unavailable_premium", "Not available: needs Microsoft Entra ID P1 or P2"),
    (
        "security.unavailable_role",
        "Not available: your role can't read this (for example Reports Reader or Global Reader can)",
    ),
    ("security.unavailable", "Not available: {error}"),
    ("security.reregister", "Require MFA re-registration…"),
    ("security.delete_method", "Delete method"),
    ("security.details", "Details…"),
    ("method.password", "Password"),
    ("method.password_details", "Can't be shown or removed; reset it instead"),
    ("method.authenticator", "Microsoft Authenticator"),
    ("method.app_version", "App version {value}"),
    ("method.mobile_phone", "Mobile phone"),
    ("method.alternate_mobile_phone", "Alternate mobile phone"),
    ("method.office_phone", "Office phone"),
    ("method.phone", "Phone"),
    ("method.ending", "Ending {number}"),
    ("method.sms_sign_in", "SMS sign-in enabled"),
    ("method.passkey", "Passkey (FIDO2)"),
    ("method.model", "Model {value}"),
    ("method.windows_hello", "Windows Hello for Business"),
    ("method.key_strength", "Key strength {value}"),
    ("method.email", "Email (password reset)"),
    ("method.oath", "Authenticator app code (OATH)"),
    ("method.minutes", "{count} minutes"),
    ("method.one_use", "one use"),
    ("method.usable_now", "usable now"),
    ("method.not_usable", "not usable: {reason}"),
    ("method.tap", "Temporary Access Pass"),
    ("method.platform_credential", "Platform credential"),
    ("method.external", "External MFA"),
    (
        "method.delete_phone",
        "They can no longer use this number for MFA or password reset. A number that is their default MFA method can't be deleted until they choose another default; a mobile number can't be deleted while an alternate mobile number remains.",
    ),
    (
        "method.delete_authenticator",
        "Notifications and codes from the app on that device stop working. They can register the app again.",
    ),
    ("method.delete_passkey", "They can no longer sign in with this security key or passkey."),
    (
        "method.delete_windows_hello",
        "Windows Hello sign-in on that device stops working until it is set up again.",
    ),
    ("method.delete_email", "They can no longer use this address to reset their password."),
    ("method.delete_tap", "The pass can no longer be used to sign in."),
    ("method.delete_other", "They can no longer sign in with it."),
    ("method.password_not_deletable", "The password can't be deleted. Use Reset password to change it."),
    (
        "method.not_deletable",
        "{method} can't be deleted with Microsoft Graph. Remove it in the Microsoft Entra admin centre.",
    ),
    ("method.delete_question", "Delete {method} from {name}?"),
    ("method.delete_title", "Delete authentication method"),
    ("method.deleting", "Deleting authentication method…"),
    ("method.deleted", "Deleted {method} from {name}"),
    ("col.method", "Method"),
    ("col.details", "Details"),
    ("col.added_utc", "Added (UTC)"),
    ("col.date_utc", "Date (UTC)"),
    ("col.application", "Application"),
    ("col.ip_address", "IP address"),
    ("col.location", "Location"),
    ("col.conditional_access", "Conditional Access"),
    // Sign-in error codes
    ("advice.50126", "Wrong user name or password. If the user is sure of their password, reset it."),
    (
        "advice.50053",
        "The account is locked after too many failed attempts (smart lockout), or sign-ins from that IP address are blocked. It unlocks by itself after the lockout period. Many attempts the user didn't make can mean someone is guessing the password.",
    ),
    ("advice.50057", "The account is disabled. Enable it if the user should be able to sign in."),
    ("advice.50055", "The password has expired. Reset it, or have the user change it."),
    ("advice.50144", "The user's Active Directory password has expired. Change it on-premises."),
    (
        "advice.50056",
        "No password is stored for this user in Microsoft Entra ID. Set one, or check password hash synchronisation for a synced user.",
    ),
    ("advice.50034", "No account with that user name is in this directory: check the name the user types."),
    (
        "advice.50074",
        "MFA was required for this sign-in. This is normal, and is usually followed by a successful sign-in.",
    ),
    (
        "advice.500121",
        "MFA failed: the user denied or didn't answer the prompt, or entered a wrong code. Prompts the user didn't expect can mean someone else knows their password.",
    ),
    (
        "advice.50072",
        "The user has to register for MFA before they can continue. A Temporary Access Pass lets them sign in to register.",
    ),
    (
        "advice.53003",
        "Blocked by Conditional Access. The sign-in's details list the policies that applied, if this app can read Conditional Access data.",
    ),
    (
        "advice.53000",
        "Conditional Access requires a compliant device, and this device isn't marked compliant.",
    ),
    ("advice.53001", "Conditional Access requires a Microsoft Entra hybrid joined device."),
    ("advice.50097", "Device authentication is required for this sign-in."),
    (
        "advice.50105",
        "The user isn't assigned to this application, which requires assignment. Assign them under Applications.",
    ),
    (
        "advice.50133",
        "The session is no longer valid because the password changed or sessions were revoked. Signing in again fixes it.",
    ),
    (
        "advice.70044",
        "The session expired, or a Conditional Access sign-in frequency setting asked for a new sign-in. Signing in again fixes it.",
    ),
    ("advice.50140", "The \"Stay signed in?\" question interrupted the sign-in. This is normal."),
    (
        "advice.50158",
        "An external security challenge, such as a third-party MFA provider, wasn't completed.",
    ),
    ("advice.50199", "The user was asked to confirm the sign-in for security reasons. This is normal."),
    (
        "advice.50058",
        "Silent sign-in found no session to use. The app then usually asks the user to sign in, which is normal.",
    ),
    (
        "advice.65001",
        "The user, or an administrator, hasn't consented to the permissions the application asks for.",
    ),
    ("advice.90094", "The application needs an administrator to grant consent."),
    (
        "advice.50020",
        "The account's identity provider isn't known to this tenant: often a guest who wasn't invited or has been removed.",
    ),
    // Sign-ins
    ("signins.success", "Success"),
    ("signins.interrupted", "Interrupted ({code})"),
    ("signins.failed", "Failed ({code})"),
    ("signins.ca_passed", "Passed"),
    ("signins.ca_failed", "Failed"),
    ("signins.ca_not_applied", "Not applied"),
    ("signins.details_title", "Sign-in details"),
    ("signins.date", "Date (UTC): {value}"),
    ("signins.user", "User: {value}"),
    ("signins.application", "Application: {value}"),
    ("signins.resource", "Resource: {value}"),
    ("signins.result", "Result: {value}"),
    ("signins.reason", "Reason: {value}"),
    ("signins.additional", "Additional details: {value}"),
    ("signins.meaning", "What it means: {value}"),
    ("signins.interactive", "Interactive: {value}"),
    ("signins.client_app", "Client app: {value}"),
    ("signins.ip", "IP address: {value}"),
    ("signins.location", "Location: {value}"),
    ("signins.device", "Device: {value}"),
    ("signins.device_state", "Device compliant: {compliant} · managed: {managed}"),
    ("signins.conditional_access", "Conditional Access: {value}"),
    ("signins.requires", "requires {grants}"),
    ("signins.no_policies", "Policies: none applied"),
    ("signins.policies", "Policies:"),
    (
        "signins.policies_hidden",
        "Policies: not shown. Microsoft Graph only lists them when this app has Policy.Read.ConditionalAccess and your role can read Conditional Access (Global Reader, Security Reader, Security Administrator or Conditional Access Administrator).",
    ),
    ("signins.risk", "Risk during sign-in: {risk} ({state})"),
    ("signins.correlation_id", "Correlation ID: {value}"),
    ("signins.request_id", "Request ID: {value}"),
    // Troubleshooting
    ("diagnose.disabled", "The account is disabled, so the user can't sign in."),
    (
        "diagnose.disabled_synced",
        "The account is disabled, so the user can't sign in. It is synced from Active Directory: enable it there.",
    ),
    (
        "diagnose.synced",
        "The account is synced from on-premises Active Directory, so its password and whether it is enabled are set there (unless password writeback is set up).",
    ),
    (
        "diagnose.guest",
        "This is a guest: they sign in with their home organisation's account, whose password and MFA methods are managed there. Revoking sessions here doesn't sign them out of their home tenant.",
    ),
    (
        "diagnose.no_mfa",
        "No MFA method is registered. Where MFA is required, the user is asked to register at their next sign-in, or blocked if your policies don't allow registering from where they are. A Temporary Access Pass lets them sign in to register.",
    ),
    ("diagnose.tap_usable", "The user has a Temporary Access Pass (usable now), created {created}."),
    (
        "diagnose.tap_unusable",
        "The user has a Temporary Access Pass (not usable yet or any more), created {created}.",
    ),
    (
        "diagnose.no_sign_ins",
        "There are no interactive sign-ins in the sign-in log for this user (it keeps 30 days).",
    ),
    ("diagnose.latest_succeeded", "The latest sign-in, at {time}, succeeded."),
    (
        "diagnose.last_failed",
        "The last failed sign-in was at {time} to {application}: error {code}, {reason}.",
    ),
    (
        "diagnose.bad_passwords",
        "{count} of the recent sign-ins failed with a wrong password: the user may have forgotten it, or someone may be guessing it.",
    ),
    (
        "diagnose.revoked",
        "Sessions from before {time} UTC are no longer valid (sessions were revoked then).",
    ),
    ("diagnose.no_problems", "No problems found."),
    // Overview
    ("overview.account", "Account"),
    ("overview.enabled", "Enabled"),
    ("overview.disabled", "Disabled: the user can't sign in"),
    ("overview.last_interactive", "Last interactive sign-in"),
    ("overview.last_non_interactive", "Last non-interactive sign-in"),
    ("overview.last_successful", "Last successful sign-in"),
    ("overview.password_expiry", "Password expiry"),
    ("overview.never_expires", "Never expires"),
    ("overview.sessions_valid_from", "Sessions valid from"),
    ("overview.mfa_registered", "Registered for MFA"),
    ("overview.mfa_capable", "Can use MFA"),
    ("overview.default_mfa", "Default MFA method"),
    ("overview.passwordless", "Passwordless capable"),
    ("overview.sspr_registered", "Registered for password reset"),
    ("overview.registration_updated", "Registration details updated"),
    ("overview.recent_sign_ins", "Recent sign-ins"),
    ("overview.troubleshooting", "Troubleshooting"),
    // Revoking sessions and Temporary Access Passes
    ("revoke.title", "Revoke sessions"),
    (
        "revoke.question",
        "Revoke all sign-in sessions for {name}?\n\nThis invalidates the refresh tokens and browser session cookies issued to them, so their apps and browsers have to sign in again the next time they need a new token. It can take a few minutes to take effect.\n\nIt does not end access straight away: access tokens already issued stay valid until they expire, usually within an hour (apps that support Continuous Access Evaluation may stop sooner). It also doesn't stop them signing in again: disable the account or reset the password for that.",
    ),
    (
        "revoke.guest",
        "This is a guest, who signs in through their home organisation, so this doesn't revoke their sessions there.",
    ),
    ("revoke.working", "Revoking sessions…"),
    ("revoke.done", "Sessions revoked for {name}"),
    ("tap.title", "Temporary Access Pass — {name}"),
    (
        "tap.intro",
        "A Temporary Access Pass is a time-limited passcode the user can sign in with, for example to set up Microsoft Authenticator or a passkey, or after losing their phone. Creating one replaces any pass they already have. It must be enabled in the tenant's authentication methods policy, which also limits its lifetime and whether it can be used more than once. The pass is shown only once.",
    ),
    ("tap.lifetime", "Lifetime in minutes"),
    ("tap.once", "Can be used only once"),
    ("tap.start", "Start (UTC, optional)"),
    ("tap.start_note", "Leave Start blank for now, or enter a time such as 2026-12-24 09:00."),
    ("tap.bad_lifetime", "Enter the lifetime as a number of minutes."),
    ("tap.lifetime_range", "The lifetime must be between 10 minutes and 43,200 minutes (30 days)."),
    ("tap.bad_start", "Enter the start as a date and time, such as 2026-12-24 09:00."),
    ("tap.creating", "Creating Temporary Access Pass…"),
    ("tap.done", "Temporary Access Pass created for {name}"),
    (
        "tap.valid_now",
        "This pass for {name} is valid for {minutes} minutes from now. It won't be shown again: give it to the user in a secure way.",
    ),
    (
        "tap.valid_now_once",
        "This pass for {name} is valid for {minutes} minutes from now, for one sign-in. It won't be shown again: give it to the user in a secure way.",
    ),
    (
        "tap.valid_from",
        "This pass for {name} is valid for {minutes} minutes from {start} UTC. It won't be shown again: give it to the user in a secure way.",
    ),
    (
        "tap.valid_from_once",
        "This pass for {name} is valid for {minutes} minutes from {start} UTC, for one sign-in. It won't be shown again: give it to the user in a secure way.",
    ),
    ("tap.not_usable", "It can't be used yet: {reason}."),
    // MFA re-registration
    ("reregister.title", "Require MFA re-registration"),
    (
        "reregister.none",
        "{name} has no MFA methods registered, so they will already be asked to register where MFA is required.",
    ),
    (
        "reregister.intro",
        "Microsoft Graph has no single switch for this. As Microsoft documents, it is done by deleting the user's MFA methods; for {name} these {count} will be deleted:\n\n{methods}\n\nThe next time they sign in where MFA is required, they have to register a method again (a Temporary Access Pass can help). Until then, sign-ins that need MFA prompt for registration, or are blocked if your policies don't allow registering from where they are.\n\nKept: the password, Windows Hello for Business and platform credentials on their devices, the email address used for password reset, and any Temporary Access Pass. Delete those on the Authentication methods tab too if a device may be compromised. A phone number that is their default MFA method may refuse to be deleted; each result is shown afterwards.",
    ),
    ("reregister.form_title", "Require MFA re-registration — {name}"),
    ("reregister.delete", "Delete methods"),
    ("reregister.revoke", "Also revoke sign-in sessions, so apps have to sign in again"),
    ("reregister.working", "Deleting MFA methods…"),
    ("reregister.not_deletable", "can't be deleted with Microsoft Graph"),
    ("reregister.revoke_step", "Revoke sign-in sessions"),
    ("reregister.done_title", "MFA re-registration required"),
    ("reregister.done", "All {count} steps succeeded for {name}."),
    ("reregister.incomplete_title", "MFA re-registration incomplete"),
    (
        "reregister.incomplete",
        "{failed} of {count} steps failed for {name}, so they may still be able to use a method that wasn't deleted.",
    ),
    // Workflows and wizards
    ("workflow.skipped_because", "Not run, because \"{step}\" did not succeed"),
    ("workflow.not_run", "Not run"),
    ("workflow.working_step", "Working… step {step} of {count}"),
    ("workflow.completed", "Completed: all {count} steps succeeded."),
    (
        "workflow.not_completed",
        "Not completed: {done} succeeded, {failed} failed, {skipped} not run. Review the failures below.",
    ),
    ("workflow.waiting", "Waiting"),
    ("workflow.running", "Running…"),
    ("workflow.done", "Done"),
    ("workflow.failed", "Failed"),
    ("workflow.dont_close", "Working… don't close the app until every step has finished."),
    ("workflow.save_report", "Save report…"),
    ("workflow.save_report_title", "Save report"),
    ("workflow.saved_report", "Saved the report to {path}"),
    ("workflow.copy_report", "Copy report"),
    ("workflow.copied_report", "Copied the report to the clipboard"),
    ("col.step", "Step"),
    ("col.action", "Action"),
    ("wizard.page_results", "Results"),
    ("wizard.back", "Back"),
    ("wizard.next", "Next"),
    // Onboarding
    ("onboard.title", "Onboard a new user"),
    ("onboard.task", "Onboard {upn}"),
    ("onboard.page_user", "User"),
    ("onboard.page_groups", "Groups"),
    ("onboard.page_review", "Review"),
    ("onboard.group_dynamic", "Dynamic membership: set by its rule"),
    ("onboard.group_synced", "Synced from Active Directory: change it there"),
    (
        "onboard.group_exchange",
        "Distribution list or mail-enabled security group: add the user in Exchange Online once they have a mailbox",
    ),
    ("onboard.group_roles", "Role-assignable: its members get directory roles, so it isn't ticked for you"),
    ("onboard.group_licences", "Assigns licences"),
    ("onboard.no_alias", "Enter a user name."),
    (
        "onboard.alias_char",
        "The user name can't contain '{char}'. Use letters, digits and ' . - _ ! # ^ ~ only; choose the domain separately.",
    ),
    ("onboard.alias_dots", "The user name can't start or end with a full stop, or have two in a row."),
    ("onboard.password_length", "The password must be 8 to 256 characters."),
    (
        "onboard.password_classes",
        "The password needs three of: upper case letters, lower case letters, digits and symbols.",
    ),
    ("onboard.step_create", "Create the user {upn}"),
    ("onboard.step_manager", "Set their manager to {name}"),
    ("onboard.step_licence", "Assign the licence {name}"),
    ("onboard.step_group", "Add them to the group {name}"),
    ("onboard.no_display_name", "Enter a display name."),
    ("onboard.no_domain", "Choose or enter the domain."),
    ("onboard.bad_usage_location", "The usage location is a two-letter country code, such as GB or US."),
    (
        "onboard.upn_taken",
        "A user or object named {upn} already exists. Go back and choose another user name.",
    ),
    ("onboard.upn_unknown", "Could not check whether {upn} is free: {error}"),
    (
        "onboard.no_licence",
        "No licence is assigned, so the user won't get a mailbox or Microsoft 365 apps until one is.",
    ),
    (
        "onboard.role_group_ticked",
        "A role-assignable group is ticked: the user will get the directory roles assigned to it.",
    ),
    (
        "onboard.no_force_change",
        "The user won't be asked to change the initial password when they first sign in.",
    ),
    ("onboard.created", "Created with object ID {id}"),
    ("onboard.manager_set", "Manager set"),
    ("onboard.licence_assigned", "Licence assigned"),
    ("onboard.pick_group", "Add the new user to a group"),
    ("onboard.reading_group", "Reading group…"),
    ("onboard.add_group_title", "Add group"),
    ("onboard.copy_from", "Copy group memberships from"),
    ("onboard.reading_groups_of", "Reading {name}'s groups…"),
    ("onboard.copied_groups", "Copied {count} group memberships from {name}"),
    ("onboard.copy_groups_title", "Copy group memberships"),
    ("onboard.checking_upn", "Checking the user name…"),
    (
        "onboard.usage_location_note",
        "A two-letter country code such as GB, needed before licences can be assigned.",
    ),
    ("onboard.manager", "Manager"),
    ("onboard.none", "None"),
    ("onboard.choose", "Choose…"),
    ("onboard.clear", "Clear"),
    ("onboard.initial_password", "Initial password"),
    ("onboard.show", "Show"),
    ("onboard.generate", "Generate"),
    ("onboard.generate_tip", "Make a new random password"),
    ("onboard.force_change", "Require a password change at first sign-in"),
    ("onboard.licences_failed", "Could not load the licences: {error}"),
    ("onboard.licences_later", "You can go on without assigning any, and assign them later."),
    ("onboard.no_licences", "The tenant has no licences for users."),
    ("onboard.licences_intro", "Tick the licences to assign. Each is assigned with all its services."),
    ("onboard.sku_available", "{sku} · {count} available"),
    (
        "onboard.groups_intro",
        "Choose the groups to add the user to, or copy them from a user in a similar role. Groups whose membership is set elsewhere are listed but can't be ticked.",
    ),
    ("onboard.add_group", "Add group…"),
    ("onboard.copy_from_user", "Copy from user…"),
    ("onboard.no_groups", "No groups chosen."),
    ("onboard.password_change", "Password change at first sign-in"),
    ("onboard.required", "Required"),
    ("onboard.not_required", "Not required"),
    ("onboard.steps_intro", "These steps will run, in this order:"),
    ("onboard.checking_free", "Checking the user name is free…"),
    ("onboard.upn_free", "{upn} is free."),
    (
        "onboard.give_password",
        "Give {upn} this initial password in a secure way. It isn't saved in the report.",
    ),
    ("onboard.copy_password", "Copy password"),
    ("onboard.report_title", "Onboarding {upn}"),
    ("onboard.report_file", "onboarding report"),
    ("onboard.create", "Create user"),
    (
        "onboard.licence_needs_location",
        "Licences can only be assigned to a user with a usage location. Go back and enter one.",
    ),
    ("onboard.pick_manager", "Manager of the new user"),
    // Offboarding
    ("offboard.title", "Offboard user"),
    ("offboard.task", "Offboard {name}"),
    ("offboard.page_choose", "Choose what to do"),
    ("offboard.page_preview", "Preview"),
    ("offboard.disable", "Block sign-in (disable the account)"),
    ("offboard.step_role", "Remove the directory role {name}"),
    ("offboard.step_eligible", "Remove eligibility for the directory role {name} (PIM)"),
    (
        "offboard.step_eligible_active",
        "Remove eligibility for the directory role {name} and end its activation (PIM)",
    ),
    ("offboard.step_convert", "Convert the mailbox to a shared mailbox"),
    ("offboard.step_forward", "Forward email to {address}"),
    ("offboard.step_forward_copy", "Forward email to {address}, keeping a copy"),
    ("offboard.step_group", "Remove from the group {name}"),
    ("offboard.step_licence", "Remove the licence {name}"),
    ("offboard.step_delete", "Delete the user account"),
    ("offboard.wait", "Wait until the user's details have loaded."),
    (
        "offboard.delete_keeps_mailbox",
        "A shared mailbox belongs to its user account, so deleting the account deletes the mailbox and stops forwarding. Keep the account to keep the mailbox, or untick the mailbox steps.",
    ),
    ("offboard.bad_forward", "Enter the address to forward email to, such as manager@contoso.com."),
    ("offboard.forward_to_self", "Email can't be forwarded to the same mailbox."),
    ("offboard.nothing_chosen", "Choose at least one thing to do."),
    (
        "offboard.warn_synced",
        "The account is synced from Active Directory: blocking sign-in and deleting it fail here, and would be undone by the next sync. Do those in Active Directory.",
    ),
    (
        "offboard.warn_not_converted",
        "The mailbox isn't being converted, so once its licence goes it is disabled, and Exchange Online deletes it after 30 days. Convert it to a shared mailbox to keep it.",
    ),
    (
        "offboard.warn_mailbox_unchecked",
        "The mailbox couldn't be checked. If the user has one, it is disabled once its licence goes and deleted after 30 days: check it under Exchange Online before going on.",
    ),
    (
        "offboard.warn_large_mailbox",
        "A shared mailbox over 50 GB, or with an archive or a litigation hold, still needs a licence. If this one does, keep its Exchange Online licence.",
    ),
    ("offboard.warn_licences_wait", "Licences are only removed if the mailbox steps succeed."),
    (
        "offboard.warn_revoke",
        "Revoking sessions makes apps sign in again for a new token, which can take a few minutes to take effect. Access tokens already issued stay valid until they expire, usually within an hour.",
    ),
    ("offboard.warn_stays_in_group", "Stays in {group} ({note})."),
    ("offboard.warn_licence_stays", "{licence} stays, as it comes from the group {groups}."),
    (
        "offboard.warn_delete",
        "Deleting the account removes it from every group, role and application. It can be restored from Deleted users for 30 days, then it is gone for good. It only runs if every other step succeeds.",
    ),
    (
        "offboard.warn_guest",
        "This is a guest: revoking sessions doesn't sign them out of their home organisation.",
    ),
    ("offboard.blocked", "Sign-in blocked"),
    ("offboard.revoked", "Sessions revoked; existing access tokens last until they expire"),
    ("offboard.removed", "Removed"),
    ("offboard.request", "Request {status}"),
    ("offboard.activation_removal", "activation removal {status}"),
    ("offboard.converted", "Converted to a shared mailbox"),
    ("offboard.forwarding", "Forwarding to {address}"),
    ("offboard.deleted", "Deleted; it can be restored for 30 days"),
    ("offboard.groups_unread", "The groups couldn't be read, so none are removed: {error}"),
    (
        "offboard.mailbox_unread",
        "The mailbox couldn't be checked, so the mailbox steps are unavailable: {error}",
    ),
    ("offboard.roles_unread", "The directory roles couldn't be read, so none are removed: {error}"),
    (
        "offboard.activations_unread",
        "Roles activated through PIM couldn't be told apart, so removing one may fail and stop the account being deleted: {error}",
    ),
    ("offboard.active_now", "{role}, active now"),
    ("offboard.scoped", "{role} (scope {scope})"),
    ("offboard.group_dynamic", "Dynamic membership, set by its rule"),
    ("offboard.group_synced", "Synced from Active Directory, so it is changed there"),
    ("offboard.group_exchange", "Changed in Exchange Online"),
    ("offboard.not_yourself", "You can't offboard the account you are signed in with."),
    ("offboard.reading", "Reading {name}…"),
    ("offboard.preview_intro", "These changes will be made to {upn}, in this order:"),
    ("offboard.only_if_all", "only if all {count} steps before it succeed"),
    ("offboard.only_if_step", "only if step {step} succeeds"),
    ("offboard.only_if_steps", "only if steps {steps} succeed"),
    ("offboard.before_you_go_on", "Before you go on"),
    ("offboard.checked", "I have checked these changes"),
    ("offboard.checked_delete", "I have checked these changes, including deleting the account"),
    ("offboard.report_title", "Offboarding {upn}"),
    ("offboard.report_file", "offboarding report"),
    ("offboard.run", "Run {count} steps"),
    ("offboard.reading_all", "Reading {name}'s groups, roles, licences and mailbox…"),
    ("offboard.read_failed", "Could not read the user: {error}"),
    ("offboard.account_enabled", "account enabled"),
    ("offboard.account_disabled", "account already disabled"),
    ("offboard.section_sign_in", "Sign-in"),
    ("offboard.no_roles", "No directory roles."),
    ("offboard.remove_active_role", "Remove {role} (active)"),
    ("offboard.remove_eligible_role", "Remove {role} (eligible (PIM))"),
    ("offboard.section_mailbox", "Mailbox"),
    ("offboard.no_mailbox", "No Exchange Online mailbox."),
    ("offboard.mailbox_unchecked", "Couldn't check the mailbox: {error}"),
    ("offboard.convert", "Convert to a shared mailbox, so it can be kept without a licence"),
    ("offboard.already_kind", "The mailbox is already of the type {kind}."),
    (
        "offboard.full_access_note",
        "To give someone access to the mailbox, add Full Access on its Mailbox permissions tab under Exchange Online.",
    ),
    ("offboard.no_groups", "Not a member of any group."),
    ("offboard.select_all", "Select all"),
    ("offboard.select_none", "Select none"),
    ("offboard.no_licences", "No licences."),
    ("offboard.remove_licence", "Remove {licence}"),
    ("offboard.inherited_licence", "{licence}: from the group {groups}, so it goes when they leave it"),
    ("offboard.delete_account", "Delete the user account (it can be restored for 30 days)"),
    (
        "offboard.delete_unavailable",
        "Unavailable while the mailbox is kept or forwarded, as deleting the account deletes its mailbox.",
    ),
    // Signing in and Microsoft Graph
    (
        "auth.needs_exchange",
        "This app has not been granted the Exchange.Manage permission on Office 365 Exchange Online, which managing Exchange needs. An administrator can add it to the app registration's API permissions and grant admin consent.",
    ),
    ("auth.needs_graph", "This app has not been granted the Microsoft Graph permissions it asks for."),
    (
        "auth.needs_scopes",
        "This needs Microsoft Graph permissions that haven't been granted to this app: {scopes}. An administrator can approve them by signing in when asked, or by adding them to the app registration's API permissions and granting admin consent.",
    ),
    ("auth.cancelled", "The sign-in was cancelled."),
    ("auth.expired", "The sign-in code expired. Choose File > Sign in to try again."),
    ("auth.refresh_failed", "Could not refresh the sign-in ({status}): {error}"),
    ("auth.more_failed", "Could not ask for more permissions ({status}): {error}"),
    ("auth.not_signed_in", "Not signed in. Choose File > Sign in."),
    ("graph.throttled", "{service} is throttling requests; try again shortly."),
    // Updates
    ("update.github_error", "GitHub answered {status}."),
    ("update.checking", "Checking for updates…"),
    ("update.check_title", "Check for updates"),
    ("update.latest", "You have the latest version, {version}."),
    // Remote servers
    ("server.add_title", "Add server"),
    ("server.add", "Add"),
    (
        "server.form_intro",
        "The server is checked over SSH with read-only commands; nothing is run with sudo or changed. Passwords and key passphrases are never saved: you are asked for one the first time the server is checked, and it is kept until you close the app.",
    ),
    ("server.field_host", "Host name or IP address"),
    ("server.field_port", "SSH port"),
    ("server.field_user", "User name"),
    ("server.field_name", "Name in the list (optional)"),
    ("server.field_auth", "Sign in with"),
    ("server.field_key", "Private key file (for an SSH key)"),
    ("server.field_passphrase", "Passphrase"),
    ("server.auth_password", "Password"),
    ("server.auth_key", "SSH key"),
    ("server.auth_key_path", "SSH key ({path})"),
    ("server.bad_host", "Enter a host name or IP address, without spaces or slashes."),
    ("server.bad_port", "The port must be a number from 1 to 65535."),
    ("server.no_key_file", "There is no key file at {path}."),
    ("server.save_title", "Save the server list"),
    ("server.remove_title", "Remove server"),
    ("server.remove_text", "Remove {name} from the list? Nothing on the server is changed."),
    ("server.removed", "Removed {name} from the list"),
    ("server.checking", "Checking {name}"),
    ("server.checked", "Checked {name}"),
    ("server.sign_in_title", "Sign in to {name}"),
    ("server.sign_in", "Sign in"),
    ("server.password_intro", "Enter the password for {account}. It is kept only until you close the app."),
    (
        "server.passphrase_intro",
        "The key {path} has a passphrase. Enter it to sign in as {account}. It is kept only until you close the app.",
    ),
    ("server.password_rejected", "The server did not accept the password."),
    ("server.key_rejected", "The server did not accept the SSH key. Check that its public key is in the user's authorized_keys file."),
    ("server.bad_passphrase", "Could not open the key {path}; check the passphrase. ({error})"),
    ("server.bad_key", "Could not open the key {path}: {error}"),
    ("server.needs_secret", "A password or passphrase is needed."),
    ("server.address", "{host} port {port}"),
    ("server.could_not_connect", "Could not connect to {address}"),
    ("server.connect_timed_out", "{address} did not answer in time."),
    ("server.check_timed_out", "The server took too long to answer."),
    ("server.too_much_output", "The server sent far more than expected, so the check was stopped."),
    (
        "server.no_shell",
        "The server did not run the check. It needs a Unix shell (sh), as Linux servers have. Exit status {exit}: {error}",
    ),
    ("server.host_key_unknown", "The server's host key is not trusted yet."),
    (
        "server.host_key_changed",
        "The host key of {address} is not the one recorded in {path} (line {line}). Someone could be pretending to be the server, so the app has not connected. If the server's key was changed on purpose, remove the old line from that file and check again.",
    ),
    ("server.trust_title", "Trust this server?"),
    (
        "server.trust_text",
        "The app has not connected to {address} before. Its host key is:\n\n{algorithm}\n{fingerprint}\n\nTrust it only if this matches the server's own key, which \"ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub\" shows on the server. Trusting it adds it to the app's known_hosts file.",
    ),
    ("server.state_ok", "OK"),
    ("server.state_checking", "Checking…"),
    ("server.state_failed", "Failed: {error}"),
    ("server.state_not_checked", "Not checked"),
    ("server.not_checked_yet", "Not checked yet. Choose Refresh to check the server."),
    ("server.up_to_date", "Up to date"),
    ("server.updates_count", "{count}"),
    ("server.updates_security", "{count} ({security} security)"),
    ("server.updates_unknown", "Not known"),
    (
        "server.updates_not_listed",
        "The server's pending updates could not be listed. Its package manager may not be supported, or the listing failed.",
    ),
    ("server.uptime", "{days} d {hours} h"),
    // (end of catalogue)
];

/// Languages offered: code, name in the language itself, and name in English. Only languages
/// written in the Latin, Greek and Cyrillic alphabets are listed, because the window cannot yet
/// draw other scripts, or text that runs from right to left.
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en", "English", "English"),
    ("cy", "Cymraeg", "Welsh"),
    ("ga", "Gaeilge", "Irish"),
    ("gd", "Gàidhlig", "Scottish Gaelic"),
    ("cs", "Čeština", "Czech"),
    ("da", "Dansk", "Danish"),
    ("de", "Deutsch", "German"),
    ("el", "Ελληνικά", "Greek"),
    ("es", "Español", "Spanish"),
    ("fr", "Français", "French"),
    ("hr", "Hrvatski", "Croatian"),
    ("it", "Italiano", "Italian"),
    ("lt", "Lietuvių", "Lithuanian"),
    ("lv", "Latviešu", "Latvian"),
    ("hu", "Magyar", "Hungarian"),
    ("nl", "Nederlands", "Dutch"),
    ("nb", "Norsk bokmål", "Norwegian"),
    ("pl", "Polski", "Polish"),
    ("pt", "Português", "Portuguese"),
    ("ro", "Română", "Romanian"),
    ("sk", "Slovenčina", "Slovak"),
    ("sl", "Slovenščina", "Slovenian"),
    ("sq", "Shqip", "Albanian"),
    ("fi", "Suomi", "Finnish"),
    ("sv", "Svenska", "Swedish"),
    ("tr", "Türkçe", "Turkish"),
    ("uk", "Українська", "Ukrainian"),
    ("bg", "Български", "Bulgarian"),
    ("ru", "Русский", "Russian"),
    ("sr", "Српски", "Serbian"),
];

/// The translation in use, or `None` for English.
static ACTIVE: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

thread_local! {
    /// Set while `in_english` runs on this thread.
    static ENGLISH_ONLY: Cell<bool> = const { Cell::new(false) };
}

fn english_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| ENGLISH.iter().copied().collect())
}

/// The English text for `key`.
pub fn english(key: &str) -> Option<&'static str> {
    english_map().get(key).copied()
}

/// The text for `key` in the language in use.
pub fn t(key: &str) -> String {
    tf(key, &[])
}

/// The text for `key` in the language in use, with each `{name}` replaced by its value from
/// `args`.
pub fn tf(key: &str, args: &[(&str, &dyn Display)]) -> String {
    if ENGLISH_ONLY.get() {
        return fill(template(None, key), args);
    }
    // The lock is let go before the values are filled in: a value can be an error whose text
    // comes from here too, and reading the lock again while the window switches language would
    // wait for ever.
    let text = template(ACTIVE.read().unwrap_or_else(|e| e.into_inner()).as_ref(), key).to_owned();
    fill(&text, args)
}

/// Whether a translation is in use, rather than English.
pub fn translated() -> bool {
    ACTIVE.read().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Runs `f` with all interface text in English on this thread, for what has to read the same
/// whatever the interface language, such as the rows and column names of a database export.
pub fn in_english<T>(f: impl FnOnce() -> T) -> T {
    /// Puts the setting back however `f` ends, so a panic can't leave the window in English.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            ENGLISH_ONLY.set(self.0);
        }
    }
    let _restore = Restore(ENGLISH_ONLY.replace(true));
    f()
}

/// The text for `key` before values are filled in.
fn template<'a>(active: Option<&'a HashMap<String, String>>, key: &'a str) -> &'a str {
    match active.and_then(|a| a.get(key)) {
        Some(text) => text.as_str(),
        None => english(key).unwrap_or_else(|| {
            log::warn!("no English text for {key}");
            key
        }),
    }
}

/// Replaces each `{name}` in `template` with its value. Braces that are not a known name are left
/// as they are.
fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after
            .find('}')
            .and_then(|end| args.iter().find(|(name, _)| *name == &after[..end]).map(|(_, v)| (end, v)));
        match value {
            Some((end, v)) => {
                out.push_str(&v.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The names in braces in `text`, such as `name` in "Opening {name}."
fn placeholders(text: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if end > 0 && after[..end].chars().all(|c| c.is_ascii_lowercase() || c == '_') => {
                names.insert(&after[..end]);
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    names
}

/// Characters that could hide or disguise text: control characters, and the marks that reorder
/// text or make it invisible. They could be used to make a button or a warning read as something
/// it isn't.
fn is_hidden_char(c: char) -> bool {
    (c.is_control() && c != '\n')
        || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
}

/// Whether a translation of `key` can be used: it is not empty or unreasonably long, has no
/// hidden characters, only has line breaks where the English does, and has the same `{name}`
/// values as the English.
fn fits(key: &str, text: &str) -> bool {
    let Some(en) = english(key) else { return false };
    let limit = en.chars().count() * 4 + 40;
    !text.trim().is_empty()
        && text.chars().count() <= limit
        && !text.chars().any(is_hidden_char)
        && (en.contains('\n') || !text.contains('\n'))
        && placeholders(en) == placeholders(text)
}

/// A language name from a translation file, if it is safe to show.
fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let ok = !name.is_empty() && name.chars().count() <= MAX_NAME_CHARS && !name.chars().any(|c| is_hidden_char(c) || c == '\n');
    ok.then(|| name.to_owned())
}

// ----- languages and translation files -------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    /// The language's name in that language, such as "Français".
    pub name: String,
    /// The language's name in English, such as "French".
    pub english_name: String,
}

impl Language {
    /// How the language is shown in the list, such as "Français (French)".
    pub fn label(&self) -> String {
        if self.name == self.english_name || self.english_name.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.english_name)
        }
    }
}

/// A translation file: the language's names, which model made it, and the text for each key.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Translation {
    pub language: String,
    pub english_name: String,
    /// The AI model that made the translation, if any.
    pub made_with: String,
    pub strings: BTreeMap<String, String>,
}

impl Translation {
    pub fn new(language: &Language) -> Self {
        Self { language: language.name.clone(), english_name: language.english_name.clone(), ..Self::default() }
    }

    /// Keys with no usable translation, in catalogue order.
    pub fn missing(&self) -> Vec<&'static str> {
        ENGLISH
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| !self.strings.get(*key).is_some_and(|text| fits(key, text)))
            .collect()
    }

    /// The translations that can be used: known keys whose `{name}` values match the English.
    /// The rest are left out, so they show in English.
    fn usable(&self) -> HashMap<String, String> {
        self.strings.iter().filter(|(key, text)| fits(key, text)).map(|(key, text)| (key.clone(), text.clone())).collect()
    }
}

/// Translations of the interface, one JSON file per language, next to the settings.
pub fn languages_dir() -> PathBuf {
    crate::config::config_dir().join("languages")
}

pub fn file_for(dir: &Path, code: &str) -> PathBuf {
    dir.join(format!("{code}.json"))
}

/// A language code that is safe to use as a file name.
fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 16 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn load(dir: &Path, code: &str) -> anyhow::Result<Translation> {
    if !valid_code(code) {
        bail!("\"{code}\" is not a language code");
    }
    let path = file_for(dir, code);
    let size = std::fs::metadata(&path).context("the file could not be read")?.len();
    if size > MAX_FILE_BYTES {
        bail!("the file is too large (the limit is 1 MB)");
    }
    let text = std::fs::read_to_string(&path).context("the file could not be read")?;
    serde_json::from_str(&text).context("the file is not a valid translation")
}

pub fn save(dir: &Path, code: &str, translation: &Translation) -> anyhow::Result<()> {
    if !valid_code(code) {
        bail!("\"{code}\" is not a language code");
    }
    let json = serde_json::to_vec_pretty(translation)?;
    std::fs::create_dir_all(dir).context("the languages folder could not be created")?;
    // Written to a new file and then swapped in, so a crash can't leave a half-written
    // translation, and a link in its place is replaced rather than followed.
    let path = file_for(dir, code);
    let temp = dir.join(format!(".{code}.json.tmp"));
    let _ = std::fs::remove_file(&temp);
    std::fs::write(&temp, json).context("the translation could not be saved")?;
    std::fs::rename(&temp, &path).context("the translation could not be saved")
}

/// The languages to offer: English, the built-in list, and any other translation files someone
/// has put in the languages folder.
pub fn available(dir: &Path) -> Vec<Language> {
    let mut languages: Vec<Language> = LANGUAGES
        .iter()
        .map(|(code, name, english_name)| Language {
            code: (*code).into(),
            name: (*name).into(),
            english_name: (*english_name).into(),
        })
        .collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return languages };
    let mut extra = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(code) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if languages.iter().any(|l| l.code == code) {
            continue;
        }
        match load(dir, code) {
            Ok(t) => match clean_name(&t.language) {
                Some(name) => extra.push(Language {
                    code: code.to_owned(),
                    name,
                    english_name: clean_name(&t.english_name).unwrap_or_default(),
                }),
                None => log::warn!("Translation {code} has no usable language name"),
            },
            Err(e) => log::warn!("Could not read translation {code}: {e:#}"),
        }
    }
    extra.sort_by_key(|l| l.name.to_lowercase());
    languages.extend(extra);
    languages
}

/// Uses `translation` for all interface text, or English if `None`.
pub fn activate(translation: Option<&Translation>) {
    let usable = translation.map(Translation::usable);
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = usable;
}

/// Switches to the saved language at startup. Falls back to English, with a note in the log, if
/// its translation can't be read.
pub fn activate_saved(dir: &Path, code: &str) {
    if code == ENGLISH_CODE || code.is_empty() {
        return;
    }
    match load(dir, code) {
        Ok(translation) => activate(Some(&translation)),
        Err(e) => log::warn!("Could not load translation {code}, using English: {e:#}"),
    }
}

// ----- translating with a local AI model ------------------------------------

fn ollama() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // A large model can take minutes over a batch on a laptop.
            .timeout(Duration::from_secs(900))
            .build()
            .expect("HTTP client")
    })
}

/// The models installed in Ollama, by name.
pub fn models() -> anyhow::Result<Vec<String>> {
    let body: Value = ollama()
        .get(format!("{OLLAMA_URL}/api/tags"))
        .send()
        .context("Ollama did not answer. Check that it is running.")?
        .json()
        .context("Ollama sent an unexpected reply")?;
    let mut names: Vec<String> = body["models"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_owned))
        // Embedding models can't write text.
        .filter(|n| !n.contains("embed"))
        .collect();
    names.sort();
    Ok(names)
}

/// Asks the Ollama model `model` for a JSON answer to `prompt`.
fn generate(model: &str, prompt: &str) -> anyhow::Result<String> {
    let request = json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "format": "json",
        // Keep answers steady, so the same text gets the same translation. Ollama's default
        // context can be as small as 2,048 tokens, which a batch of text and its translation can
        // overflow, cutting the answer short.
        "options": { "temperature": 0.1, "num_ctx": 8192 },
    });
    // Ollama explains a failure, such as a model it can't load, in the reply.
    let body: Value = ollama()
        .post(format!("{OLLAMA_URL}/api/generate"))
        .json(&request)
        .send()
        .context("Ollama did not answer. Check that it is running.")?
        .json()
        .context("Ollama sent an unexpected reply")?;
    if let Some(error) = body["error"].as_str() {
        bail!("Ollama could not use the model {model}: {error}");
    }
    match body["response"].as_str() {
        Some(text) if !text.trim().is_empty() => Ok(text.to_owned()),
        _ => bail!("the AI model gave an empty answer"),
    }
}

fn prompt(language: &Language, batch: &BTreeMap<&str, &str>) -> String {
    let target = if language.english_name.is_empty() {
        language.name.clone()
    } else {
        format!("{} ({})", language.english_name, language.name)
    };
    let input = serde_json::to_string_pretty(batch).unwrap_or_default();
    format!(
        "You are translating the interface of a console that IT administrators use to manage Microsoft Entra ID \
         (users, groups, devices, licences and Exchange Online). Translate each value in the JSON object below from \
         British English into {target}.\n\n\
         Rules:\n\
         - Reply with only a JSON object that has exactly the same keys, with each value translated.\n\
         - Keep every name in curly braces, such as {{name}} or {{count}}, exactly as it is. Do not translate it.\n\
         - Keep these unchanged: key names such as Ctrl, Shift, Tab, Enter, Escape, Delete, Insert, F1, F2, F5, F6 and \
           F10; product and service names such as Microsoft Entra ID, Microsoft Graph, Microsoft 365, Exchange Online, \
           Office 365, Teams, My Apps, Temporary Access Pass, MariaDB, Ollama and GitHub; permission names such as \
           User.ReadWrite.All or Exchange.Manage; property names such as isAssignableToRole; and abbreviations such \
           as CSV, TLS, MFA, UPN, SKU, ID, GUID, URL and OS.\n\
         - Keep line breaks, quotation marks and the … character where the English has them.\n\
         - Use the words Microsoft uses in that language for Microsoft Entra ID and Microsoft 365, and the form of \
           address usual for software in that language.\n\
         - Translate every value, even short ones.\n\n\
         For example, into French, {{\"menu.file\": \"File\", \"status.loading_list\": \"Loading {{list}}…\"}} \
         becomes {{\"menu.file\": \"Fichier\", \"status.loading_list\": \"Chargement des {{list}}…\"}}\n\n\
         {input}"
    )
}

/// Translates one batch of keys with the Ollama model `model`. Returns the translations that can
/// be used; any the model got wrong are left out.
pub fn translate_batch(model: &str, language: &Language, keys: &[&'static str]) -> anyhow::Result<BTreeMap<String, String>> {
    let batch: BTreeMap<&str, &str> = keys.iter().filter_map(|k| Some((*k, english(k)?))).collect();
    let reply = generate(model, &prompt(language, &batch))?;
    let reply: Value = serde_json::from_str(reply.trim()).context("the AI model did not reply with a translation")?;
    let Some(reply) = reply.as_object() else { bail!("the AI model did not reply with a translation") };
    let mut out = BTreeMap::new();
    for key in keys {
        match reply.get(*key).and_then(Value::as_str) {
            // Small models sometimes hand the English back. Leaving it out shows the same text,
            // and lets the next run try it again.
            Some(text) if english(key) == Some(text.trim()) => log::debug!("{key} came back untranslated"),
            Some(text) if fits(key, text) => {
                out.insert((*key).to_owned(), text.trim().to_owned());
            }
            Some(_) => log::debug!("Translation of {key} dropped: placeholders changed"),
            None => log::debug!("Translation of {key} missing from the reply"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn render(active: Option<&HashMap<String, String>>, key: &str, args: &[(&str, &dyn Display)]) -> String {
        fill(template(active, key), args)
    }

    #[test]
    fn keys_are_unique() {
        assert_eq!(english_map().len(), ENGLISH.len(), "a key appears twice in ENGLISH");
    }

    const SOURCES: [&str; 18] = [
        include_str!("app.rs"),
        include_str!("auth.rs"),
        include_str!("bulk.rs"),
        include_str!("db.rs"),
        include_str!("dialogs.rs"),
        include_str!("exchange.rs"),
        include_str!("graph.rs"),
        include_str!("language.rs"),
        include_str!("model.rs"),
        include_str!("offboarding.rs"),
        include_str!("onboarding.rs"),
        include_str!("ops.rs"),
        include_str!("security.rs"),
        include_str!("servers.rs"),
        include_str!("sheets.rs"),
        include_str!("update.rs"),
        include_str!("workflow.rs"),
        include_str!("main.rs"),
    ];

    /// The sections keys are grouped in, such as `menu` in `menu.file`.
    fn sections() -> BTreeSet<&'static str> {
        ENGLISH.iter().filter_map(|(key, _)| key.split('.').next()).collect()
    }

    #[test]
    fn every_key_in_the_code_has_english_text() {
        // Keys are passed to t() and tf(), and also kept in tables (such as column titles) and
        // looked up when shown, so look for any quoted string that starts with a section name.
        let quoted = keys_in();
        let mut count = 0;
        for source in SOURCES {
            for key in quoted(source) {
                count += 1;
                assert!(english(key).is_some(), "no English text for {key}");
            }
        }
        assert!(count > 1000, "only found {count} keys in the code");
    }

    /// The string literals in Rust source, skipping escaped quotes and the `'"'` character.
    fn literals(source: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let bytes = source.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"'\"'") {
                i += 3;
            } else if bytes[i..].starts_with(b"'\\\"'") {
                i += 4;
            } else if bytes[i] == b'"' {
                let start = i + 1;
                i = start;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                out.push(&source[start..i.min(bytes.len())]);
                i += 1;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Finds quoted strings such as "menu.file" whose first part is a section of `ENGLISH`.
    fn keys_in() -> impl Fn(&str) -> Vec<&str> {
        let sections = sections();
        move |source: &str| {
            literals(source)
                .into_iter()
                .filter(|piece| {
                    piece.contains('.')
                        && piece.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
                        && piece.split('.').next().is_some_and(|s| sections.contains(s))
                })
                .collect()
        }
    }

    #[test]
    fn finds_literals_past_escaped_quotes() {
        let source = r#"a("x \"y\" z"); b('"'); c("menu.file");"#;
        assert_eq!(literals(source), ["x \\\"y\\\" z", "menu.file"]);
    }

    #[test]
    fn every_english_text_is_used() {
        for (key, _) in ENGLISH {
            let quoted = format!("\"{key}\"");
            assert!(SOURCES.iter().any(|s| s.contains(&quoted)), "{key} is not used anywhere");
        }
    }

    #[test]
    fn english_has_no_hidden_characters() {
        for (key, text) in ENGLISH {
            assert!(!text.chars().any(is_hidden_char), "{key} has a hidden character");
            assert!(fits(key, text), "{key} does not fit its own checks");
        }
    }

    #[test]
    fn fills_in_values() {
        assert_eq!(fill("Opening {name}.", &[("name", &"notes")]), "Opening notes.");
        // Unknown names and stray braces are left alone.
        assert_eq!(fill("{a} {b} {", &[("a", &1)]), "1 {b} {");
    }

    #[test]
    fn uses_the_translation_and_falls_back_to_english() {
        let fr = map(&[("menu.file", "Fichier")]);
        assert_eq!(render(Some(&fr), "menu.file", &[]), "Fichier");
        assert_eq!(render(Some(&fr), "menu.help", &[]), "Help");
    }

    #[test]
    fn english_scope_ignores_the_translation() {
        let mut fr = Translation::default();
        fr.strings.insert("menu.file".into(), "Fichier".into());
        // Only this test's thread sees the scope, but other tests share ACTIVE, so check the
        // scope against a key no other test translates.
        fr.strings.insert("menu.view".into(), "Affichage".into());
        activate(Some(&fr));
        assert_eq!(in_english(|| t("menu.view")), "View");
        assert_eq!(t("menu.view"), "Affichage");
        activate(None);
    }

    #[test]
    fn finds_placeholders() {
        assert_eq!(placeholders("{a} and {b_c}, not {Up} or {}"), BTreeSet::from(["a", "b_c"]));
    }

    #[test]
    fn drops_translations_that_lose_or_change_values() {
        let mut t = Translation::default();
        t.strings.insert("status.loading_list".into(), "Chargement des {liste}…".into());
        t.strings.insert("menu.file".into(), "Fichier".into());
        t.strings.insert("menu.help".into(), "   ".into());
        t.strings.insert("no.such.key".into(), "Rien".into());
        let usable = t.usable();
        assert_eq!(usable.len(), 1);
        assert_eq!(usable["menu.file"], "Fichier");
        let missing = t.missing();
        assert!(missing.contains(&"status.loading_list"));
        assert!(missing.contains(&"menu.help"));
        assert!(!missing.contains(&"menu.file"));
        assert_eq!(missing.len(), ENGLISH.len() - 1);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stormcloud-i18n-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saves_loads_and_lists_translations() {
        let dir = temp_dir("list");
        let mut fr = Translation::new(&Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() });
        fr.strings.insert("menu.file".into(), "Fichier".into());
        save(&dir, "fr", &fr).unwrap();
        assert_eq!(load(&dir, "fr").unwrap().strings["menu.file"], "Fichier");

        // A file for a language not in the built-in list is offered too.
        let eo = Translation { language: "Esperanto".into(), ..Translation::default() };
        save(&dir, "eo", &eo).unwrap();
        let languages = available(&dir);
        assert_eq!(languages[0].code, ENGLISH_CODE);
        assert!(languages.iter().any(|l| l.code == "eo" && l.label() == "Esperanto"));
        assert_eq!(languages.iter().filter(|l| l.code == "fr").count(), 1);

        assert!(load(&dir, "../settings").is_err());
        assert!(save(&dir, "", &eo).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_hidden_characters_and_overlong_text() {
        assert!(fits("menu.file", "Fichier"));
        // Right-to-left override, which can make text read backwards.
        assert!(!fits("menu.file", "Fich\u{202e}ier"));
        assert!(!fits("menu.file", "Fich\u{200b}ier"));
        assert!(!fits("menu.file", "Fich\u{7}ier"));
        assert!(!fits("menu.file", "Fich\nier"));
        assert!(!fits("menu.file", &"x".repeat(100)));
        assert_eq!(clean_name("  Esperanto "), Some("Esperanto".into()));
        assert_eq!(clean_name("Esper\u{202e}anto"), None);
        assert_eq!(clean_name(&"x".repeat(61)), None);
    }

    #[test]
    fn refuses_files_that_are_too_large() {
        let dir = temp_dir("large");
        let big = format!("{{\"language\": \"Big\", \"made_with\": \"{}\"}}", "x".repeat(MAX_FILE_BYTES as usize));
        std::fs::write(file_for(&dir, "xx"), big).unwrap();
        assert!(load(&dir, "xx").is_err());
        assert!(available(&dir).iter().all(|l| l.code != "xx"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_languages() {
        let fr = Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() };
        assert_eq!(fr.label(), "Français (French)");
        let en = Language { code: "en".into(), name: "English".into(), english_name: "English".into() };
        assert_eq!(en.label(), "English");
    }

    #[test]
    fn the_prompt_keeps_braces() {
        let fr = Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() };
        let batch = BTreeMap::from([("status.loading_list", "Loading {list}…")]);
        let p = prompt(&fr, &batch);
        assert!(p.contains("such as {name} or {count}"));
        assert!(p.contains("French (Français)"));
        assert!(p.contains("\"status.loading_list\": \"Loading {list}…\""));
    }

    /// Needs Ollama running. Uses the model named in `STORMCLOUD_OLLAMA_MODEL`, or gemma3:4b.
    #[test]
    #[ignore]
    fn translates_with_a_local_model() {
        let model = std::env::var("STORMCLOUD_OLLAMA_MODEL").unwrap_or_else(|_| "gemma3:4b".into());
        let fr = Language { code: "fr".into(), name: "Français".into(), english_name: "French".into() };
        let keys: Vec<&str> = ENGLISH.iter().map(|(k, _)| *k).take(BATCH_SIZE).collect();
        let done = translate_batch(&model, &fr, &keys).unwrap();
        for (key, text) in &done {
            println!("{key}: {text}");
        }
        assert!(done.len() >= BATCH_SIZE * 3 / 4, "only {} of {BATCH_SIZE} came back usable", done.len());
    }
}
