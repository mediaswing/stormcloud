//! Console tree nodes, their Graph endpoints and result-pane columns.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Root,
    Users,
    Groups,
    Devices,
    Applications,
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
    (Node::Exchange, 1),
    (Node::Mailboxes, 2),
    (Node::DistributionLists, 2),
    (Node::Licences, 1),
];

pub struct Column {
    pub title: &'static str,
    pub width: f32,
    pub get: fn(&Value) -> String,
}

impl Node {
    pub fn title(self) -> &'static str {
        match self {
            Node::Root => "Microsoft Entra ID",
            Node::Users => "Users",
            Node::Groups => "Groups",
            Node::Devices => "Devices",
            Node::Applications => "Applications",
            Node::Exchange => "Exchange Online",
            Node::Mailboxes => "Mailboxes",
            Node::DistributionLists => "Distribution lists",
            Node::Licences => "Licences",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Node::Root => "🏢",
            Node::Users => "👤",
            Node::Groups => "👥",
            Node::Devices => "💻",
            Node::Applications => "🧩",
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

    /// Singular noun for the objects listed under this node.
    pub fn noun(self) -> &'static str {
        match self {
            Node::Users => "user",
            Node::Groups => "group",
            Node::Devices => "device",
            Node::Applications => "application",
            Node::Mailboxes => "mailbox",
            Node::DistributionLists => "distribution list",
            Node::Licences => "licence",
            Node::Root | Node::Exchange => "item",
        }
    }

    pub fn endpoint(self) -> Option<Source> {
        Some(Source::Graph(match self {
            Node::Users => "/users?$select=id,displayName,userPrincipalName,mail,accountEnabled,userType,jobTitle,department,usageLocation,createdDateTime,assignedLicenses&$top=999",
            Node::Groups => "/groups?$select=id,displayName,description,mail,mailEnabled,mailNickname,securityEnabled,groupTypes,isAssignableToRole,membershipRule,createdDateTime,assignedLicenses&$top=999",
            Node::Devices => "/devices?$select=id,deviceId,displayName,operatingSystem,operatingSystemVersion,trustType,accountEnabled,approximateLastSignInDateTime,isCompliant,isManaged,registrationDateTime&$top=999",
            Node::Applications => "/servicePrincipals?$select=id,appId,displayName,publisherName,appOwnerOrganizationId,servicePrincipalType,accountEnabled,appRoleAssignmentRequired,tags,appRoles,homepage&$top=999",
            Node::Licences => "/subscribedSkus",
            Node::Mailboxes | Node::DistributionLists => return Some(Source::Exchange),
            Node::Root | Node::Exchange => return None,
        }))
    }

    pub fn columns(self) -> Vec<Column> {
        fn c(title: &'static str, width: f32, get: fn(&Value) -> String) -> Column {
            Column { title, width, get }
        }
        match self {
            Node::Users => vec![
                c("Display name", 200.0, |v| s(v, "displayName")),
                c("User principal name", 260.0, |v| s(v, "userPrincipalName")),
                c("Enabled", 70.0, |v| yes_no(&v["accountEnabled"])),
                c("Type", 80.0, |v| s(v, "userType")),
                c("Job title", 140.0, |v| s(v, "jobTitle")),
                c("Department", 140.0, |v| s(v, "department")),
                c("Licences", 70.0, |v| count(&v["assignedLicenses"])),
            ],
            Node::Groups => vec![
                c("Display name", 220.0, |v| s(v, "displayName")),
                c("Type", 120.0, group_type),
                c("Membership", 90.0, |v| membership(v).into()),
                c("Role-assignable", 110.0, |v| yes_no(&v["isAssignableToRole"])),
                c("Email", 220.0, |v| s(v, "mail")),
                c("Description", 260.0, |v| s(v, "description")),
            ],
            Node::Devices => vec![
                c("Name", 200.0, |v| s(v, "displayName")),
                c("OS", 90.0, |v| s(v, "operatingSystem")),
                c("Version", 120.0, |v| s(v, "operatingSystemVersion")),
                c("Join type", 130.0, |v| join_type(&s(v, "trustType")).into()),
                c("Enabled", 70.0, |v| yes_no(&v["accountEnabled"])),
                c("Compliant", 80.0, |v| yes_no(&v["isCompliant"])),
                c("Managed", 80.0, |v| yes_no(&v["isManaged"])),
                c("Last sign-in", 140.0, |v| date(&v["approximateLastSignInDateTime"])),
            ],
            Node::Applications => vec![
                c("Name", 220.0, |v| s(v, "displayName")),
                c("Publisher", 180.0, |v| s(v, "publisherName")),
                c("Enabled", 70.0, |v| yes_no(&v["accountEnabled"])),
                c("Assignment required", 130.0, |v| yes_no(&v["appRoleAssignmentRequired"])),
                c("Visible to users", 110.0, |v| yes_no(&visible_to_users(v))),
                c("Application ID", 280.0, |v| s(v, "appId")),
            ],
            Node::Licences => vec![
                c("Product", 260.0, |v| sku_name(&s(v, "skuPartNumber"))),
                c("SKU", 200.0, |v| s(v, "skuPartNumber")),
                c("Assigned", 80.0, |v| v["consumedUnits"].as_i64().unwrap_or(0).to_string()),
                c("Total", 80.0, |v| v["prepaidUnits"]["enabled"].as_i64().unwrap_or(0).to_string()),
                c("Available", 80.0, |v| {
                    let total = v["prepaidUnits"]["enabled"].as_i64().unwrap_or(0);
                    (total - v["consumedUnits"].as_i64().unwrap_or(0)).to_string()
                }),
                c("Status", 90.0, |v| s(v, "capabilityStatus")),
                c("Applies to", 90.0, |v| s(v, "appliesTo")),
            ],
            Node::Mailboxes => vec![
                c("Display name", 220.0, |v| s(v, "DisplayName")),
                c("Email address", 260.0, |v| s(v, "PrimarySmtpAddress")),
                c("Type", 90.0, crate::exchange::mailbox_type),
                c("Forwarding to", 220.0, crate::exchange::forwarding),
                c("Hidden", 70.0, |v| yes_no(&v["HiddenFromAddressListsEnabled"])),
            ],
            Node::DistributionLists => vec![
                c("Display name", 220.0, |v| s(v, "DisplayName")),
                c("Email address", 260.0, |v| s(v, "PrimarySmtpAddress")),
                c("Type", 160.0, crate::exchange::group_type),
                c("Internal senders only", 140.0, |v| yes_no(&v["RequireSenderAuthenticationEnabled"])),
                c("Hidden", 70.0, |v| yes_no(&v["HiddenFromAddressListsEnabled"])),
            ],
            Node::Root | Node::Exchange => vec![
                c("Name", 240.0, |v| s(v, "name")),
                c("Description", 400.0, |v| s(v, "description")),
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
            Node::Root | Node::Exchange => s(v, "name"),
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
        Some(true) => "Yes".into(),
        Some(false) => "No".into(),
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
    match (unified, security, mail) {
        (true, _, _) => "Microsoft 365",
        (false, true, true) => "Mail-enabled security",
        (false, true, false) => "Security",
        (false, false, true) => "Distribution",
        _ => "Other",
    }
    .into()
}

pub fn membership(v: &Value) -> &'static str {
    let dynamic = v["groupTypes"]
        .as_array()
        .is_some_and(|a| a.iter().any(|t| t == "DynamicMembership"));
    if dynamic {
        "Dynamic"
    } else {
        "Assigned"
    }
}

pub fn join_type(t: &str) -> &str {
    match t {
        "AzureAd" => "Entra joined",
        "ServerAd" => "Hybrid joined",
        "Workplace" => "Entra registered",
        other => other,
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

/// Flattens a JSON object into display rows for a General property page.
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
            (label.to_string(), text)
        })
        .collect()
}
