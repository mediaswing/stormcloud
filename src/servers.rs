//! The Remote Servers node: Linux and other Unix servers the administrator keeps an eye on over
//! SSH, showing each one's operating system, installed and running software and patch level.
//!
//! The list is kept in `servers.json` beside the settings, with the result of each server's last
//! check, so it shows at startup without signing in to anything. Passwords and key passphrases
//! are never written down: each is asked for the first time a server is checked, and kept in
//! memory until the app closes.
//!
//! Checking a server signs in over SSH and runs [`REPORT_SCRIPT`], which only reads: it runs
//! nothing with sudo and changes nothing on the server. What it prints is read by
//! [`parse_report`].
//!
//! A server's host key must be in the user's `~/.ssh/known_hosts` (so servers they already reach
//! with `ssh` are trusted as they are) or in the app's own `known_hosts`. An unknown key is shown
//! to be confirmed first, and a key that has changed is refused, as OpenSSH does.

use crate::app::App;
use crate::config::config_dir;
use crate::dialogs::{Dialog, Field, Form};
use crate::i18n::{t, tf};
use crate::model::{s, yes_no, Node};
use crate::sheets::{table, TabBody, TabKind};
use anyhow::{anyhow, bail, Context};
use russh::keys::{self, Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{client, ChannelMsg, Disconnect, Preferred};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 22;
/// How long to wait for the server to answer and agree keys.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a whole check may take. Listing updates on dnf can mean downloading metadata, and
/// [`REPORT_SCRIPT`] lists them twice, each with its own 100-second limit.
const CHECK_TIMEOUT: Duration = Duration::from_secs(240);
/// More output than this means something other than the report is being printed.
const MAX_OUTPUT: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    #[default]
    Password,
    Key,
}

/// How to reach a server. Rows of the list hold these fields and the last check's results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub auth: AuthMethod,
    /// The private key file, for [`AuthMethod::Key`]. `~` stands for the home folder.
    #[serde(default)]
    pub key_path: String,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

pub fn servers_path() -> PathBuf {
    config_dir().join("servers.json")
}

/// Host keys the user chose to trust in the app.
pub fn known_hosts_path() -> PathBuf {
    config_dir().join("known_hosts")
}

/// The known_hosts files a host key is looked for in.
fn known_hosts_files() -> Vec<PathBuf> {
    let mut files = vec![known_hosts_path()];
    if let Some(home) = dirs::home_dir() {
        files.push(home.join(".ssh").join("known_hosts"));
    }
    files
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// The usual key file, if there is one, to offer when adding a server.
fn default_key_path() -> String {
    let Some(home) = dirs::home_dir() else { return String::new() };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .find(|name| home.join(".ssh").join(name).is_file())
        .map(|name| format!("~/.ssh/{name}"))
        .unwrap_or_default()
}

/// The saved list, and what to tell the user when some or all of it couldn't be read. The file
/// is then copied to `servers.json.bad`, so saving the list doesn't lose what was left out.
pub fn load_rows() -> (Vec<Value>, Option<String>) {
    let path = servers_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return (Vec::new(), None) };
    let backup = path.with_extension("json.bad");
    let (rows, problem) = match serde_json::from_str::<Vec<Value>>(&text) {
        Ok(rows) => {
            let total = rows.len();
            let rows: Vec<Value> = rows.into_iter().filter(|r| serde_json::from_value::<Server>(r.clone()).is_ok()).collect();
            let skipped = total - rows.len();
            let problem = (skipped > 0).then(|| {
                tf("server.bad_entries", &[("count", &skipped), ("path", &path.display()), ("backup", &backup.display())])
            });
            (rows, problem)
        }
        Err(e) => (Vec::new(), Some(tf("server.bad_file", &[("path", &path.display()), ("error", &e), ("backup", &backup.display())]))),
    };
    if let Some(problem) = &problem {
        log::error!("{problem}");
        if let Err(e) = std::fs::copy(&path, &backup) {
            log::error!("Could not copy {} to {}: {e}", path.display(), backup.display());
        }
    }
    (rows, problem)
}

/// Saves the list, leaving out what only matters while the app runs.
fn save_rows(rows: &[Value]) -> anyhow::Result<()> {
    let rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut r = r.clone();
            if let Some(o) = r.as_object_mut() {
                o.remove("checking");
            }
            r
        })
        .collect();
    let dir = config_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = servers_path();
    let text = serde_json::to_string_pretty(&rows)?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

fn new_id() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        bytes = (nanos as u64).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn iso_utc(secs: u64) -> String {
    let (y, m, d) = crate::ops::civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn now_iso() -> String {
    iso_utc(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
}

// ----- errors that need the user ---------------------------------------------

/// The server's host key is in no known_hosts file, so it has to be confirmed.
#[derive(Debug)]
pub struct UnknownHostKey(pub PublicKey);

impl std::fmt::Display for UnknownHostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", t("server.host_key_unknown"))
    }
}
impl std::error::Error for UnknownHostKey {}

/// A password, or the key's passphrase, is needed.
#[derive(Debug)]
pub struct NeedsSecret;

impl std::fmt::Display for NeedsSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", t("server.needs_secret"))
    }
}
impl std::error::Error for NeedsSecret {}

/// The password or passphrase given was wrong.
#[derive(Debug)]
pub struct BadSecret(pub String);

impl std::fmt::Display for BadSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for BadSecret {}

// ----- connecting --------------------------------------------------------------

enum HostKeyProblem {
    Unknown(PublicKey),
    Changed { path: PathBuf, line: usize },
}

/// Accepts the server only if its host key is known, recording why not otherwise.
struct Verifier {
    host: String,
    port: u16,
    files: Vec<PathBuf>,
    problem: Arc<Mutex<Option<HostKeyProblem>>>,
}

impl client::Handler for Verifier {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => PublicKey::from(cert.public_key().clone()),
        };
        let Some(problem) = host_key_problem(&self.host, self.port, &key, &self.files) else { return Ok(true) };
        *self.problem.lock().unwrap_or_else(|e| e.into_inner()) = Some(problem);
        Ok(false)
    }
}

/// Why `key` isn't trusted for the host, or `None` when it is the known key. A different key of
/// the same type in any of the files means it has changed, which is refused even if another file
/// has the new one.
fn host_key_problem(host: &str, port: u16, key: &PublicKey, files: &[PathBuf]) -> Option<HostKeyProblem> {
    let mut known = false;
    for path in files {
        match keys::check_known_hosts_path(host, port, key, path) {
            Ok(found) => known |= found,
            Err(keys::Error::KeyChanged { line }) => return Some(HostKeyProblem::Changed { path: path.clone(), line }),
            Err(e) => log::warn!("Could not read {}: {e}", path.display()),
        }
    }
    (!known).then(|| HostKeyProblem::Unknown(key.clone()))
}

/// The host key algorithms to offer, those already in a known_hosts file for the server first,
/// so a server known by its RSA key isn't asked for an Ed25519 key that would need confirming.
fn preferred_host_keys(host: &str, port: u16, files: &[PathBuf]) -> Vec<Algorithm> {
    let known: Vec<Algorithm> = files
        .iter()
        .filter_map(|path| keys::known_hosts::known_host_keys_path(host, port, path).ok())
        .flatten()
        .map(|(_, key)| key.algorithm())
        .collect();
    // known_hosts records RSA keys as ssh-rsa, whichever hash signs with them.
    let is_known = |a: &Algorithm| known.iter().any(|k| k == a || matches!((k, a), (Algorithm::Rsa { .. }, Algorithm::Rsa { .. })));
    let (mut first, rest): (Vec<Algorithm>, Vec<Algorithm>) = Preferred::DEFAULT.key.iter().cloned().partition(|a| is_known(a));
    first.extend(rest);
    first
}

/// Records `key` as the host key for the server in the known_hosts file at `path`.
pub fn trust_host_key(server: &Server, key: &PublicKey, path: &std::path::Path) -> anyhow::Result<()> {
    keys::known_hosts::learn_known_hosts_path(&server.host, server.port, key, path)
        .with_context(|| format!("writing {}", path.display()))
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Answers each keyboard-interactive prompt with the password, for servers that ask for it that
/// way rather than with password authentication.
async fn keyboard_interactive(session: &mut client::Handle<Verifier>, user: &str, password: &str) -> anyhow::Result<bool> {
    use russh::client::KeyboardInteractiveAuthResponse as Reply;
    let mut reply = session.authenticate_keyboard_interactive_start(user, None).await?;
    // A server could keep asking; real ones ask once or twice.
    for _ in 0..5 {
        match reply {
            Reply::Success => return Ok(true),
            Reply::Failure { .. } => return Ok(false),
            Reply::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|p| if p.echo { String::new() } else { password.to_string() }).collect();
                reply = session.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(false)
}

async fn sign_in(session: &mut client::Handle<Verifier>, server: &Server, secret: Option<&str>) -> anyhow::Result<()> {
    let user = server.user.as_str();
    let accepted = match server.auth {
        AuthMethod::Password => {
            let password = secret.ok_or(NeedsSecret)?;
            session.authenticate_password(user, password).await?.success()
                || keyboard_interactive(session, user, password).await?
        }
        AuthMethod::Key => {
            let path = expand_home(&server.key_path);
            if !path.is_file() {
                bail!(tf("server.no_key_file", &[("path", &path.display())]));
            }
            let key = match keys::load_secret_key(&path, secret) {
                Ok(key) => key,
                Err(keys::Error::KeyIsEncrypted) => return Err(NeedsSecret.into()),
                Err(e) if secret.is_some() => {
                    return Err(BadSecret(tf("server.bad_passphrase", &[("path", &path.display()), ("error", &e)])).into());
                }
                Err(e) => bail!(tf("server.bad_key", &[("path", &path.display()), ("error", &e)])),
            };
            let hash = session.best_supported_rsa_hash().await?.flatten();
            session.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?.success()
        }
    };
    if accepted {
        return Ok(());
    }
    match server.auth {
        AuthMethod::Password => Err(BadSecret(t("server.password_rejected")).into()),
        AuthMethod::Key => bail!(t("server.key_rejected")),
    }
}

/// Signs in to the server, trusting the host keys in `known_hosts`, and returns what
/// [`REPORT_SCRIPT`] printed.
async fn fetch(server: &Server, secret: Option<&str>, known_hosts: Vec<PathBuf>) -> anyhow::Result<String> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 4,
        preferred: Preferred { key: preferred_host_keys(&server.host, server.port, &known_hosts).into(), ..Preferred::DEFAULT },
        ..Default::default()
    });
    let problem = Arc::new(Mutex::new(None));
    let verifier = Verifier { host: server.host.clone(), port: server.port, files: known_hosts, problem: problem.clone() };
    let address = tf("server.address", &[("host", &server.host), ("port", &server.port)]);
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, (server.host.as_str(), server.port), verifier)).await;
    let mut session = match connected {
        Err(_) => bail!(tf("server.connect_timed_out", &[("address", &address)])),
        Ok(Err(e)) => {
            return Err(match problem.lock().unwrap_or_else(|e| e.into_inner()).take() {
                Some(HostKeyProblem::Unknown(key)) => UnknownHostKey(key).into(),
                Some(HostKeyProblem::Changed { path, line }) => {
                    anyhow!(tf("server.host_key_changed", &[("address", &address), ("path", &path.display()), ("line", &line)]))
                }
                None => anyhow!(e).context(tf("server.could_not_connect", &[("address", &address)])),
            });
        }
        Ok(Ok(session)) => session,
    };
    sign_in(&mut session, server, secret).await?;

    let mut channel = session.channel_open_session().await?;
    // The script goes in on standard input, so it needs no quoting for the user's shell.
    channel.exec(true, "sh -s").await?;
    channel.data_bytes(REPORT_SCRIPT.as_bytes()).await?;
    channel.eof().await?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut exit = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => out.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => err.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
            _ => {}
        }
        if out.len() + err.len() > MAX_OUTPUT {
            bail!(t("server.too_much_output"));
        }
    }
    let _ = session.disconnect(Disconnect::ByApplication, "", "en").await;
    let text = String::from_utf8_lossy(&out).into_owned();
    if !text.contains("@@uname") {
        let said = String::from_utf8_lossy(&err);
        let said = said.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        let exit = exit.map(|c| c.to_string()).unwrap_or_default();
        bail!(tf("server.no_shell", &[("exit", &exit), ("error", &said)]));
    }
    Ok(text)
}

/// Checks a server (on a worker thread): signs in, runs the report and reads it.
pub fn check(server: &Server, secret: Option<String>) -> anyhow::Result<Report> {
    check_with(server, secret, known_hosts_files())
}

fn check_with(server: &Server, secret: Option<String>, known_hosts: Vec<PathBuf>) -> anyhow::Result<Report> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let text = runtime
        .block_on(async { tokio::time::timeout(CHECK_TIMEOUT, fetch(server, secret.as_deref(), known_hosts)).await })
        .map_err(|_| anyhow!(t("server.check_timed_out")))??;
    Ok(parse_report(&text))
}

// ----- the report --------------------------------------------------------------

/// Run with `sh -s` on the server. It only reads: nothing runs with sudo, and package lists are
/// not refreshed, so pending updates are as of the server's own last refresh. Each part starts
/// with an `@@name` line; `@@status N` gives the exit status of the command before it, where
/// that matters.
pub const REPORT_SCRIPT: &str = r#"LC_ALL=C
export LC_ALL
have() { command -v "$1" >/dev/null 2>&1; }
limit() { if have timeout; then timeout 100 "$@"; else "$@"; fi; }
echo @@uname
uname -s; uname -r; uname -m
echo @@hostname
hostname 2>/dev/null || uname -n
echo @@os-release
cat /etc/os-release 2>/dev/null || cat /usr/lib/os-release 2>/dev/null
echo @@uptime
cut -d ' ' -f 1 /proc/uptime 2>/dev/null
echo @@services
if have systemctl; then
  echo systemd
  systemctl list-units --type=service --state=running --no-legend --no-pager --plain 2>/dev/null
else
  echo processes
  ps -e -o comm= 2>/dev/null | sort -u
fi
if have dpkg-query; then
  echo @@manager
  echo apt
  echo @@packages
  dpkg-query -W -f='${db:Status-Abbrev}\t${Package}\t${Version}\n' 2>/dev/null | awk -F '\t' '$1 ~ /^ii/ { print $2 "\t" $3 }'
  echo @@changed
  stat -c %Y /var/lib/dpkg/status 2>/dev/null
  echo @@lists-updated
  stat -c %Y /var/lib/apt/lists 2>/dev/null
  if have apt; then
    echo @@updates
    limit apt list --upgradable 2>/dev/null
    echo "@@status $?"
  fi
  # reboot-required is written by update-notifier-common (Ubuntu) and unattended-upgrades;
  # without either, needrestart can compare the running kernel with the newest installed.
  echo @@reboot
  if [ -e /var/run/reboot-required ]; then
    echo yes
  elif [ -e /usr/share/update-notifier/notify-reboot-required ] || [ -e /etc/kernel/postinst.d/unattended-upgrades ]; then
    echo no
  elif [ -x /usr/sbin/needrestart ]; then
    case "$(/usr/sbin/needrestart -b -k 2>/dev/null | sed -n 's/^NEEDRESTART-KSTA: //p')" in
      1) echo no ;;
      2|3) echo yes ;;
      *) echo unknown ;;
    esac
  else
    echo unknown
  fi
elif have rpm; then
  echo @@packages
  rpm -qa --qf '%{NAME}\t%{VERSION}-%{RELEASE}.%{ARCH}\n' 2>/dev/null | grep -v '^gpg-pubkey'
  echo @@changed
  rpm -qa --qf '%{INSTALLTIME}\n' 2>/dev/null | sort -n | tail -n 1
  if have dnf || have yum; then
    if have dnf; then pm=dnf; else pm=yum; fi
    echo @@manager
    echo "$pm"
    echo @@updates
    limit "$pm" -q check-update 2>/dev/null
    echo "@@status $?"
    echo @@security
    limit "$pm" -q check-update --security 2>/dev/null
    echo "@@status $?"
    if have needs-restarting; then
      echo @@reboot-status
      needs-restarting -r >/dev/null 2>&1
      echo "@@status $?"
    fi
  elif have zypper; then
    echo @@manager
    echo zypper
    echo @@updates
    limit zypper --non-interactive --quiet list-updates 2>/dev/null
    echo "@@status $?"
    echo @@security-patches
    limit zypper --non-interactive --quiet list-patches --category security 2>/dev/null
    echo "@@status $?"
    echo @@reboot-status
    zypper needs-rebooting >/dev/null 2>&1
    echo "@@status $?"
  fi
elif have pacman; then
  echo @@manager
  echo pacman
  echo @@packages
  pacman -Q 2>/dev/null
  echo @@changed
  stat -c %Y /var/lib/pacman/local 2>/dev/null
  if have checkupdates; then
    echo @@updates
    limit checkupdates 2>/dev/null
    echo "@@status $?"
  fi
elif have apk; then
  echo @@manager
  echo apk
  echo @@packages
  apk info -v 2>/dev/null
  echo @@changed
  stat -c %Y /lib/apk/db/installed 2>/dev/null
  echo @@updates
  apk version -l '<' 2>/dev/null
  echo "@@status $?"
fi
echo @@end
"#;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Update {
    pub name: String,
    pub installed: String,
    pub available: String,
    pub security: bool,
}

/// What a check found. Anything the server didn't say is left empty or `None`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub hostname: String,
    /// The distribution's own name for itself, such as "Ubuntu 24.04.1 LTS".
    pub os: String,
    pub os_version: String,
    /// The kernel's name and release, such as "Linux 6.8.0-45-generic".
    pub kernel: String,
    pub arch: String,
    pub uptime_secs: Option<u64>,
    pub package_manager: String,
    /// Name and version of each installed package.
    pub packages: Vec<(String, String)>,
    /// Running systemd services and their descriptions, or the names of running processes
    /// on servers without systemd.
    pub services: Vec<(String, String)>,
    pub services_are_processes: bool,
    /// Pending updates, or `None` when they couldn't be listed.
    pub updates: Option<Vec<Update>>,
    /// How many of the updates are security updates, when that is known.
    pub security_updates: Option<usize>,
    pub reboot_required: Option<bool>,
    /// When the package lists were last refreshed, and the installed packages last changed.
    pub lists_updated: Option<String>,
    pub packages_changed: Option<String>,
}

#[derive(Default)]
struct Section<'a> {
    lines: Vec<&'a str>,
    status: Option<i32>,
}

fn sections(text: &str) -> HashMap<&str, Section<'_>> {
    let mut out: HashMap<&str, Section> = HashMap::new();
    let mut current = "";
    for line in text.lines() {
        if let Some(code) = line.strip_prefix("@@status ") {
            out.entry(current).or_default().status = code.trim().parse().ok();
        } else if let Some(name) = line.strip_prefix("@@") {
            current = name.trim();
            out.entry(current).or_default();
        } else {
            out.entry(current).or_default().lines.push(line);
        }
    }
    out
}

/// Reads os-release's `KEY=value` lines, with or without quotes.
fn os_release(lines: &[&str]) -> HashMap<String, String> {
    lines
        .iter()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()))
        .collect()
}

/// `name-version-rN` from apk, split into the name and the version.
fn split_apk(full: &str) -> (String, String) {
    let mut parts = full.rsplitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(release), Some(version), Some(name)) => (name.to_string(), format!("{version}-{release}")),
        _ => (full.to_string(), String::new()),
    }
}

/// `apt list --upgradable` lines, such as
/// `bash/noble-updates,noble-security 5.2.21-2ubuntu4.1 amd64 [upgradable from: 5.2.21-2ubuntu4]`.
fn apt_updates(lines: &[&str]) -> Vec<Update> {
    lines
        .iter()
        .filter_map(|line| {
            let (name, rest) = line.split_once('/')?;
            let mut words = rest.split_whitespace();
            let suites = words.next()?;
            let available = words.next()?.to_string();
            let installed = line.split_once("from: ").map(|(_, v)| v.trim_end_matches(']').trim().to_string()).unwrap_or_default();
            let security = suites.split(',').any(|s| s.ends_with("-security"));
            Some(Update { name: name.to_string(), installed, available, security })
        })
        .collect()
}

/// `dnf check-update` output: `name.arch version repo` triples, which yum may wrap onto two
/// lines, up to the "Obsoleting Packages" list.
fn dnf_updates(lines: &[&str]) -> Vec<(String, String)> {
    let mut words = Vec::new();
    for line in lines {
        if line.starts_with("Obsoleting") {
            break;
        }
        if line.trim().is_empty() || line.starts_with("Security:") || line.starts_with("Last metadata") {
            continue;
        }
        words.extend(line.split_whitespace());
    }
    words
        .chunks(3)
        .filter(|c| c.len() == 3 && c[0].contains('.'))
        .map(|c| {
            let name = c[0].rsplit_once('.').map(|(n, _)| n).unwrap_or(c[0]);
            (name.to_string(), c[1].to_string())
        })
        .collect()
}

/// The rows of one of zypper's tables, split at the bars, without the header and rule.
fn zypper_rows<'a>(lines: &[&'a str], header: &str) -> Vec<Vec<&'a str>> {
    lines
        .iter()
        .filter(|l| l.contains('|') && !l.trim_start().starts_with('-') && !l.contains(header))
        .map(|l| l.split('|').map(str::trim).collect())
        .collect()
}

fn epoch(lines: &[&str]) -> Option<String> {
    lines.iter().find_map(|l| l.trim().parse::<u64>().ok()).filter(|&s| s > 0).map(iso_utc)
}

pub fn parse_report(text: &str) -> Report {
    let parts = sections(text);
    let empty = Section::default();
    let part = |name: &str| parts.get(name).unwrap_or(&empty);
    let mut r = Report::default();

    let uname = &part("uname").lines;
    r.kernel = uname.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    r.arch = uname.get(2).map(|s| s.to_string()).unwrap_or_default();
    r.hostname = part("hostname").lines.first().map(|s| s.trim().to_string()).unwrap_or_default();
    let os = os_release(&part("os-release").lines);
    let get = |k: &str| os.get(k).cloned().unwrap_or_default();
    r.os = match get("PRETTY_NAME") {
        name if !name.is_empty() => name,
        _ => format!("{} {}", get("NAME"), get("VERSION")).trim().to_string(),
    };
    if r.os.is_empty() {
        r.os = uname.first().map(|s| s.to_string()).unwrap_or_default();
    }
    r.os_version = get("VERSION_ID");
    r.uptime_secs = part("uptime").lines.first().and_then(|l| l.trim().parse::<f64>().ok()).map(|s| s as u64);

    let services = &part("services").lines;
    r.services_are_processes = services.first().is_some_and(|l| l.trim() == "processes");
    r.services = services
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            if r.services_are_processes {
                return (l.trim().to_string(), String::new());
            }
            // unit, load, active and sub state, then the description.
            let words: Vec<&str> = l.split_whitespace().collect();
            (words[0].trim_end_matches(".service").to_string(), words.get(4..).unwrap_or_default().join(" "))
        })
        .collect();

    r.package_manager = part("manager").lines.first().map(|s| s.trim().to_string()).unwrap_or_default();
    let pm = r.package_manager.as_str();
    r.packages = part("packages")
        .lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match pm {
            "apk" => split_apk(l.trim()),
            _ => {
                let (name, version) = l.split_once(['\t', ' ']).unwrap_or((l, ""));
                (name.trim().to_string(), version.trim().to_string())
            }
        })
        .collect();
    r.packages.sort_by_cached_key(|p| p.0.to_lowercase());
    r.packages_changed = epoch(&part("changed").lines);
    r.lists_updated = epoch(&part("lists-updated").lines);

    let installed: HashMap<&str, &str> = r.packages.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let updates = part("updates");
    // A failed listing means the updates aren't known, not that there are none.
    let listed = parts.contains_key("updates")
        && match pm {
            "dnf" | "yum" => matches!(updates.status, Some(0 | 100)),
            "pacman" => matches!(updates.status, Some(0 | 2)),
            _ => updates.status == Some(0),
        };
    r.updates = listed.then(|| match pm {
        "apt" => apt_updates(&updates.lines),
        "dnf" | "yum" => {
            let security: Vec<String> = match part("security").status {
                Some(0 | 100) => dnf_updates(&part("security").lines).into_iter().map(|(n, _)| n).collect(),
                _ => Vec::new(),
            };
            dnf_updates(&updates.lines)
                .into_iter()
                .map(|(name, available)| Update {
                    installed: installed.get(name.as_str()).unwrap_or(&"").to_string(),
                    security: security.contains(&name),
                    name,
                    available,
                })
                .collect()
        }
        "zypper" => zypper_rows(&updates.lines, "Current Version")
            .into_iter()
            .filter(|c| c.len() >= 6)
            .map(|c| Update { name: c[2].into(), installed: c[3].into(), available: c[4].into(), security: false })
            .collect(),
        "pacman" => updates
            .lines
            .iter()
            .filter_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                (w.len() >= 4 && w[2] == "->").then(|| Update { name: w[0].into(), installed: w[1].into(), available: w[3].into(), security: false })
            })
            .collect(),
        "apk" => updates
            .lines
            .iter()
            .filter_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                (w.len() >= 3 && w[1] == "<").then(|| {
                    let (name, installed) = split_apk(w[0]);
                    Update { name, installed, available: w[2].into(), security: false }
                })
            })
            .collect(),
        _ => Vec::new(),
    });
    r.security_updates = match pm {
        "apt" => r.updates.as_ref().map(|u| u.iter().filter(|u| u.security).count()),
        "dnf" | "yum" if matches!(part("security").status, Some(0 | 100)) => {
            r.updates.as_ref().map(|u| u.iter().filter(|u| u.security).count())
        }
        "zypper" if part("security-patches").status == Some(0) => {
            Some(zypper_rows(&part("security-patches").lines, "Category").len())
        }
        _ => None,
    };
    r.reboot_required = match (part("reboot").lines.first().map(|l| l.trim()), part("reboot-status").status, pm) {
        (Some("yes"), _, _) => Some(true),
        (Some("no"), _, _) => Some(false),
        (_, Some(0), _) => Some(false),
        (_, Some(1), "dnf" | "yum") | (_, Some(102), "zypper") => Some(true),
        _ => None,
    };
    r
}

// ----- the list ----------------------------------------------------------------

fn report(v: &Value) -> &Value {
    &v["report"]
}

pub fn os(v: &Value) -> String {
    s(report(v), "os")
}

pub fn kernel(v: &Value) -> String {
    s(report(v), "kernel")
}

pub fn package_count(v: &Value) -> String {
    report(v)["packages"].as_array().map(|a| a.len().to_string()).unwrap_or_default()
}

pub fn service_count(v: &Value) -> String {
    report(v)["services"].as_array().map(|a| a.len().to_string()).unwrap_or_default()
}

/// Pending updates, and how many of them are for security, as the list shows them.
pub fn patch_level(v: &Value) -> String {
    let r = report(v);
    if r.is_null() {
        return String::new();
    }
    let Some(updates) = r["updates"].as_array() else { return t("server.updates_unknown") };
    match (updates.len(), r["security_updates"].as_u64()) {
        (0, _) => t("server.up_to_date"),
        (count, Some(security)) if security > 0 => tf("server.updates_security", &[("count", &count), ("security", &security)]),
        (count, _) => tf("server.updates_count", &[("count", &count)]),
    }
}

pub fn reboot_required(v: &Value) -> String {
    yes_no(&report(v)["reboot_required"])
}

pub fn last_checked(v: &Value) -> String {
    crate::model::datetime(&v["checked_at"])
}

pub fn status(v: &Value) -> String {
    if v["checking"] == true {
        return t("server.state_checking");
    }
    match v["state"].as_str() {
        Some("ok") => t("server.state_ok"),
        Some("failed") => tf("server.state_failed", &[("error", &s(v, "error"))]),
        _ => t("server.state_not_checked"),
    }
}

fn uptime(secs: u64) -> String {
    tf("server.uptime", &[("days", &(secs / 86_400)), ("hours", &(secs % 86_400 / 3600))])
}

fn sign_in_method(v: &Value) -> String {
    match v["auth"].as_str() {
        Some("key") => tf("server.auth_key_path", &[("path", &s(v, "key_path"))]),
        _ => t("server.auth_password"),
    }
}

/// The content of a tab of a server's property sheet, from its last check.
pub fn tab_body(kind: TabKind, v: &Value) -> TabBody {
    let r = report(v);
    if kind != TabKind::General && r.is_null() {
        return TabBody::Fields(vec![(t("prop.status"), t("server.not_checked_yet"))]);
    }
    // Packages and services are kept as [name, version or description] pairs.
    let pairs = |key: &str| -> Vec<Value> { r[key].as_array().cloned().unwrap_or_default() };
    let cells = |p: &Value| vec![p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string()];
    match kind {
        TabKind::Software => table(vec!["col.name", "col.version"], pairs("packages"), cells),
        TabKind::Services => table(vec!["col.name", "col.description"], pairs("services"), cells),
        TabKind::Updates => match r["updates"].as_array() {
            None => TabBody::Fields(vec![(t("prop.pending_updates"), t("server.updates_not_listed"))]),
            Some(updates) => table(
                vec!["col.name", "col.installed_version", "col.available_version", "col.security"],
                updates.clone(),
                |u| vec![s(u, "name"), s(u, "installed"), s(u, "available"), yes_no(&u["security"])],
            ),
        },
        _ => {
            let mut fields = vec![
                (t("prop.name"), s(v, "name")),
                (t("prop.host"), s(v, "host")),
                (t("prop.port"), s(v, "port")),
                (t("prop.ssh_user"), s(v, "user")),
                (t("prop.sign_in_method"), sign_in_method(v)),
                (t("prop.status"), status(v)),
                (t("prop.last_checked"), last_checked(v)),
            ];
            if !r.is_null() {
                let opt_date = |key: &str| crate::model::datetime(&r[key]);
                fields.extend([
                    (t("prop.host_name"), s(r, "hostname")),
                    (t("prop.os"), s(r, "os")),
                    (t("prop.os_version"), s(r, "os_version")),
                    (t("prop.kernel"), s(r, "kernel")),
                    (t("prop.architecture"), s(r, "arch")),
                    (t("prop.uptime"), r["uptime_secs"].as_u64().map(uptime).unwrap_or_default()),
                    (t("prop.package_manager"), s(r, "package_manager")),
                    (t("prop.installed_packages"), package_count(v)),
                    (
                        t(if r["services_are_processes"] == true { "prop.running_processes" } else { "prop.running_services" }),
                        service_count(v),
                    ),
                    (t("prop.pending_updates"), patch_level(v)),
                    (t("prop.reboot_required"), reboot_required(v)),
                    (t("prop.lists_updated"), opt_date("lists_updated")),
                    (t("prop.packages_changed"), opt_date("packages_changed")),
                ]);
            }
            TabBody::Fields(fields)
        }
    }
}

/// Whether the row still reaches the server as `server` does, whatever it is called.
fn reached_the_same(row: &Value, server: &Server) -> bool {
    serde_json::from_value::<Server>(row.clone()).is_ok_and(|now| Server { name: server.name.clone(), ..now } == *server)
}

// ----- the app -----------------------------------------------------------------

impl App {
    fn server_row(&self, id: &str) -> Option<&Value> {
        self.data.get(&Node::RemoteServers)?.iter().find(|r| r["id"] == id)
    }

    fn save_servers(&mut self) {
        let rows = self.data.get(&Node::RemoteServers).cloned().unwrap_or_default();
        if let Err(e) = save_rows(&rows) {
            self.error(&t("server.save_title"), format!("{e:#}"));
        }
    }

    /// Updates the row for a server, and any of its property sheets that are open.
    fn update_server(&mut self, change: Value) {
        let id = s(&change, "id");
        self.upsert(Node::RemoteServers, change);
        let Some(row) = self.server_row(&id).cloned() else { return };
        for d in &mut self.dialogs {
            if let Dialog::Sheet(sheet) = &mut d.dialog
                && sheet.node == Node::RemoteServers
                && sheet.obj["id"] == row["id"]
            {
                sheet.replace_obj(row.clone());
            }
        }
    }

    pub fn check_all_servers(&mut self) {
        let ids: Vec<String> = self.data.get(&Node::RemoteServers).into_iter().flatten().map(|r| s(r, "id")).collect();
        for id in ids {
            self.check_server(&id);
        }
    }

    /// Checks a server in the background, first asking for its password if it needs one.
    pub fn check_server(&mut self, id: &str) {
        let Some(row) = self.server_row(id) else { return };
        if row["checking"] == true {
            return;
        }
        let Ok(server) = serde_json::from_value::<Server>(row.clone()) else { return };
        let secret = self.ssh_secrets.get(id).cloned();
        if server.auth == AuthMethod::Password && secret.is_none() {
            return self.ask_server_secret(server, None);
        }
        self.update_server(json!({ "id": id, "checking": true }));
        log::info!("Checking {} ({}@{}:{})", server.name, server.user, server.host, server.port);
        let label = tf("server.checking", &[("name", &server.name)]);
        let worker = server.clone();
        self.spawn_offline(label, move || check(&worker, secret), move |app, r| app.server_checked(server, r));
    }

    fn server_checked(&mut self, server: Server, result: anyhow::Result<Report>) {
        let id = server.id.clone();
        let Some(row) = self.server_row(&id) else {
            // Removed while it was being checked.
            return;
        };
        if !reached_the_same(row, &server) {
            // Changed to reach another server, or in another way, while it was being checked.
            self.update_server(json!({ "id": id, "checking": false }));
            return self.check_server(&id);
        }
        let e = match result {
            Ok(report) => {
                log::info!("Checked {}: {}", server.name, report.os);
                self.update_server(json!({
                    "id": id,
                    "checking": false,
                    "state": "ok",
                    "error": null,
                    "checked_at": now_iso(),
                    "report": report,
                }));
                self.status = tf("server.checked", &[("name", &server.name)]);
                return self.save_servers();
            }
            Err(e) => e,
        };
        self.update_server(json!({ "id": id, "checking": false }));
        if let Some(UnknownHostKey(key)) = e.downcast_ref::<UnknownHostKey>() {
            return self.confirm_host_key(server, key.clone());
        }
        if e.downcast_ref::<NeedsSecret>().is_some() {
            return self.ask_server_secret(server, None);
        }
        if let Some(BadSecret(why)) = e.downcast_ref::<BadSecret>() {
            let why = why.clone();
            self.ssh_secrets.remove(&id);
            return self.ask_server_secret(server, Some(why));
        }
        let error = format!("{e:#}");
        log::error!("Checking {} failed: {error}", server.name);
        self.status = tf("status.failed", &[("title", &tf("server.checking", &[("name", &server.name)]))]);
        self.update_server(json!({ "id": id, "state": "failed", "error": error, "checked_at": now_iso() }));
        self.save_servers();
    }

    /// Asks for the server's password, or its key's passphrase, for this session.
    fn ask_server_secret(&mut self, server: Server, problem: Option<String>) {
        if let Some(&open) = self.ssh_prompts.get(&server.id)
            && self.dialogs.iter().any(|d| d.id == open)
        {
            return;
        }
        let title = tf("server.sign_in_title", &[("name", &server.name)]);
        let account = format!("{}@{}", server.user, server.host);
        let mut intro = match server.auth {
            AuthMethod::Password => tf("server.password_intro", &[("account", &account)]),
            AuthMethod::Key => tf("server.passphrase_intro", &[("account", &account), ("path", &server.key_path)]),
        };
        if let Some(problem) = problem {
            intro = format!("{problem}\n\n{intro}");
        }
        let label = t(if server.auth == AuthMethod::Password { "field.password" } else { "server.field_passphrase" });
        let id = server.id;
        let prompt_for = id.clone();
        let form = Form::new(&title, &t("server.sign_in"))
            .intro(&intro)
            .field(Field::password("secret", &label, "").required())
            .on_submit(move |app, f| {
                app.ssh_secrets.insert(id.clone(), f.raw("secret"));
                app.check_server(&id);
                Ok(())
            });
        let open = self.push(Dialog::Form(form));
        self.ssh_prompts.insert(prompt_for, open);
    }

    fn confirm_host_key(&mut self, server: Server, key: PublicKey) {
        let text = tf(
            "server.trust_text",
            &[
                ("address", &tf("server.address", &[("host", &server.host), ("port", &server.port)])),
                ("algorithm", &key.algorithm().as_str()),
                ("fingerprint", &fingerprint(&key)),
            ],
        );
        self.push(Dialog::confirm(&t("server.trust_title"), &text, move |app| match trust_host_key(&server, &key, &known_hosts_path()) {
            Ok(()) => {
                log::info!("Trusted the host key {} for {}:{}", fingerprint(&key), server.host, server.port);
                app.check_server(&server.id);
            }
            Err(e) => app.error(&t("server.trust_title"), format!("{e:#}")),
        }));
    }

    pub fn remove_server(&mut self, obj: Value) {
        let name = s(&obj, "name");
        let text = tf("server.remove_text", &[("name", &name)]);
        self.push(Dialog::confirm(&t("server.remove_title"), &text, move |app| {
            let id = s(&obj, "id");
            app.ssh_secrets.remove(&id);
            app.remove_obj(Node::RemoteServers, &id);
            app.save_servers();
            app.status = tf("server.removed", &[("name", &name)]);
        }));
    }

    /// The form for adding a server, or for changing how one is reached.
    pub fn server_form(&mut self, existing: Option<Value>) {
        let old: Option<Server> = existing.as_ref().and_then(|v| serde_json::from_value(v.clone()).ok());
        let title = match &old {
            Some(server) => tf("sheet.edit_title", &[("name", &server.name)]),
            None => t("server.add_title"),
        };
        let field = |f: fn(&Server) -> String, default: String| old.as_ref().map(f).unwrap_or(default);
        let auth_options = vec![("password".to_string(), t("server.auth_password")), ("key".to_string(), t("server.auth_key"))];
        let form = Form::new(&title, &t(if old.is_some() { "dialog.save" } else { "server.add" }))
            .intro(&t("server.form_intro"))
            .field(Field::text("host", &t("server.field_host"), &field(|s| s.host.clone(), String::new())).required())
            .field(Field::text("port", &t("server.field_port"), &field(|s| s.port.to_string(), DEFAULT_PORT.to_string())).required())
            .field(Field::text("user", &t("server.field_user"), &field(|s| s.user.clone(), String::new())).required())
            .field(Field::text("name", &t("server.field_name"), &field(|s| s.name.clone(), String::new())))
            .field(Field::choice("auth", &t("server.field_auth"), auth_options, old.as_ref().map_or(0, |s| (s.auth == AuthMethod::Key) as usize)))
            .field(Field::text("key", &t("server.field_key"), &field(|s| s.key_path.clone(), default_key_path())))
            .on_submit(move |app, f| {
                let host = f.text("host");
                if host.contains(char::is_whitespace) || host.contains('/') {
                    return Err(t("server.bad_host"));
                }
                let port = f.text("port").parse::<u16>().ok().filter(|&p| p > 0).ok_or_else(|| t("server.bad_port"))?;
                let auth = if f.choice("auth") == "key" { AuthMethod::Key } else { AuthMethod::Password };
                let key_path = f.text("key");
                if auth == AuthMethod::Key && !expand_home(&key_path).is_file() {
                    return Err(tf("server.no_key_file", &[("path", &key_path)]));
                }
                let name = match f.text("name") {
                    name if name.is_empty() => host.clone(),
                    name => name,
                };
                let server = Server {
                    id: old.as_ref().map_or_else(new_id, |s| s.id.clone()),
                    name,
                    host,
                    port,
                    user: f.text("user"),
                    auth,
                    key_path: if auth == AuthMethod::Key { key_path } else { String::new() },
                };
                let row = serde_json::to_value(&server).map_err(|e| e.to_string())?;
                match &old {
                    Some(before) => {
                        let mut row = row;
                        if (Server { name: before.name.clone(), ..server.clone() }) != *before {
                            // The last check was of another server, or as another user.
                            app.ssh_secrets.remove(&server.id);
                            for key in ["state", "error", "checked_at", "report"] {
                                row[key] = Value::Null;
                            }
                        }
                        app.update_server(row);
                        app.save_servers();
                    }
                    None => {
                        app.data.entry(Node::RemoteServers).or_default();
                        app.upsert(Node::RemoteServers, row);
                        app.save_servers();
                        log::info!("Added the server {} ({}:{})", server.name, server.host, server.port);
                        app.check_server(&server.id);
                    }
                }
                Ok(())
            });
        self.push(Dialog::Form(form));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = "@@uname\nLinux\n6.8.0-45-generic\nx86_64\n@@hostname\nweb1\n@@os-release\n\
        PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\n@@uptime\n90061.52\n\
        @@services\nsystemd\nssh.service loaded active running OpenBSD Secure Shell server\n\
        cron.service loaded active running Regular background program processing daemon\n\
        @@manager\napt\n@@packages\nbash\t5.2.21-2ubuntu4\nopenssl\t3.0.13-0ubuntu3.3\n\
        @@changed\n1760000000\n@@lists-updated\n1760086400\n@@updates\nListing...\n\
        bash/noble-updates,noble-security 5.2.21-2ubuntu4.1 amd64 [upgradable from: 5.2.21-2ubuntu4]\n\
        vim/noble-updates 2:9.1.0016-1ubuntu7.4 amd64 [upgradable from: 2:9.1.0016-1ubuntu7.3]\n\
        @@status 0\n@@reboot\nyes\n@@end\n";

    #[test]
    fn reads_an_ubuntu_report() {
        let r = parse_report(UBUNTU);
        assert_eq!(r.os, "Ubuntu 24.04.1 LTS");
        assert_eq!(r.os_version, "24.04");
        assert_eq!(r.kernel, "Linux 6.8.0-45-generic");
        assert_eq!(r.arch, "x86_64");
        assert_eq!(r.hostname, "web1");
        assert_eq!(r.uptime_secs, Some(90061));
        assert_eq!(r.packages.len(), 2);
        assert_eq!(r.services[0], ("ssh".to_string(), "OpenBSD Secure Shell server".to_string()));
        let updates = r.updates.unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0], Update { name: "bash".into(), installed: "5.2.21-2ubuntu4".into(), available: "5.2.21-2ubuntu4.1".into(), security: true });
        assert!(!updates[1].security);
        assert_eq!(r.security_updates, Some(1));
        assert_eq!(r.reboot_required, Some(true));
        assert_eq!(r.packages_changed.as_deref(), Some("2025-10-09T08:53:20Z"));
    }

    #[test]
    fn reads_dnf_updates_and_reboot_status() {
        let text = "@@uname\nLinux\n5.14.0-427.el9.x86_64\nx86_64\n@@os-release\nPRETTY_NAME=\"Rocky Linux 9.4 (Blue Onyx)\"\n\
            @@services\nsystemd\n@@packages\nopenssl-libs\t3.0.7-27.el9.x86_64\nkernel-core\t5.14.0-427.el9.x86_64\n\
            @@changed\n1700000000\n@@manager\ndnf\n@@updates\n\n\
            openssl-libs.x86_64                1:3.0.7-28.el9_4                baseos\n\
            a-very-long-package-name-that-wraps.noarch\n                          1.2-3.el9          appstream\n\
            Obsoleting Packages\nfoo.x86_64 1-1 baseos\n@@status 100\n\
            @@security\nopenssl-libs.x86_64 1:3.0.7-28.el9_4 baseos\n@@status 100\n\
            @@reboot-status\n@@status 1\n@@end\n";
        let r = parse_report(text);
        let updates = r.updates.unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].name, "openssl-libs");
        assert_eq!(updates[0].installed, "3.0.7-27.el9.x86_64");
        assert!(updates[0].security);
        assert_eq!(updates[1].name, "a-very-long-package-name-that-wraps");
        assert_eq!(r.security_updates, Some(1));
        assert_eq!(r.reboot_required, Some(true));
    }

    #[test]
    fn failed_listing_leaves_updates_unknown() {
        let r = parse_report("@@uname\nLinux\n6.1\naarch64\n@@manager\ndnf\n@@updates\n@@status 1\n@@end\n");
        assert_eq!(r.updates, None);
        assert_eq!(r.security_updates, None);
        assert_eq!(r.reboot_required, None);
    }

    #[test]
    fn reads_alpine_without_systemd() {
        let text = "@@uname\nLinux\n6.6.31-0-lts\nx86_64\n@@os-release\nNAME=\"Alpine Linux\"\nVERSION_ID=3.20.0\n\
            @@services\nprocesses\nnginx\nsshd\n@@manager\napk\n@@packages\nmusl-1.2.5-r0\nbusybox-1.36.1-r29\n\
            @@updates\nInstalled:                                Available:\nbusybox-1.36.1-r29 < 1.36.1-r30\n@@status 0\n@@end\n";
        let r = parse_report(text);
        assert_eq!(r.os, "Alpine Linux");
        assert!(r.services_are_processes);
        assert_eq!(r.services.len(), 2);
        assert_eq!(r.packages[0], ("busybox".to_string(), "1.36.1-r29".to_string()));
        let updates = r.updates.unwrap();
        assert_eq!(updates, vec![Update { name: "busybox".into(), installed: "1.36.1-r29".into(), available: "1.36.1-r30".into(), security: false }]);
    }

    #[test]
    fn reads_zypper_tables() {
        let text = "@@uname\nLinux\n6.4\nx86_64\n@@manager\nzypper\n@@updates\n\
            S | Repository | Name | Current Version | Available Version | Arch\n\
            --+------------+------+-----------------+-------------------+-------\n\
            v | repo-sle-update | curl | 8.0.1-1.1 | 8.0.1-2.1 | x86_64\n@@status 0\n\
            @@security-patches\nRepository | Name | Category | Severity | Interactive | Status | Summary\n\
            -----------+------+----------+----------+-------------+--------+--------\n\
            repo-sle-update | SUSE-2026-1 | security | important | --- | needed | Security update for curl\n@@status 0\n\
            @@reboot-status\n@@status 102\n@@end\n";
        let r = parse_report(text);
        assert_eq!(r.updates.unwrap()[0].available, "8.0.1-2.1");
        assert_eq!(r.security_updates, Some(1));
        assert_eq!(r.reboot_required, Some(true));
    }

    #[test]
    fn summarises_the_patch_level() {
        let row = |report: Value| json!({ "id": "a", "report": report });
        assert_eq!(patch_level(&json!({ "id": "a" })), "");
        assert_eq!(patch_level(&row(json!({ "updates": null }))), "Not known");
        assert_eq!(patch_level(&row(json!({ "updates": [] }))), "Up to date");
        assert_eq!(patch_level(&row(json!({ "updates": [{}, {}], "security_updates": 1 }))), "2 (1 security)");
        assert_eq!(patch_level(&row(json!({ "updates": [{}], "security_updates": null }))), "1");
    }

    #[test]
    fn rows_round_trip_without_the_session_state() {
        let server = Server { id: "1".into(), name: "web".into(), host: "web.example.org".into(), port: 22, user: "admin".into(), auth: AuthMethod::Key, key_path: "~/.ssh/id_ed25519".into() };
        let mut row = serde_json::to_value(&server).unwrap();
        row["checking"] = json!(true);
        assert_eq!(serde_json::from_value::<Server>(row).unwrap(), server);
        // Older or hand-written entries may leave out the port and sign-in method.
        let minimal: Server = serde_json::from_value(json!({ "id": "2", "name": "db", "host": "db", "user": "root" })).unwrap();
        assert_eq!((minimal.port, minimal.auth), (22, AuthMethod::Password));
    }

    #[test]
    fn host_keys_are_checked_against_known_hosts() {
        let dir = std::env::temp_dir().join(format!("stormcloud-known-hosts-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("known_hosts");
        let a = keys::parse_public_key_base64("AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ").unwrap();
        let b = keys::parse_public_key_base64("AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X").unwrap();
        let files = vec![file.clone()];
        assert!(matches!(host_key_problem("web", 22, &a, &files), Some(HostKeyProblem::Unknown(_))));
        keys::known_hosts::learn_known_hosts_path("web", 22, &a, &file).unwrap();
        assert!(host_key_problem("web", 22, &a, &files).is_none());
        assert!(matches!(host_key_problem("web", 22, &b, &files), Some(HostKeyProblem::Changed { .. })));
        // The same host on another port is a different server.
        assert!(matches!(host_key_problem("web", 2222, &a, &files), Some(HostKeyProblem::Unknown(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Checks a real server, set with STORMCLOUD_TEST_SSH (`user@host:port`) and
    /// STORMCLOUD_TEST_SSH_KEY (a private key file without a passphrase), or
    /// STORMCLOUD_TEST_SSH_PASSWORD. The host key is trusted in a temporary file.
    #[test]
    #[ignore = "needs an SSH server"]
    fn checks_a_real_server() {
        let target = std::env::var("STORMCLOUD_TEST_SSH").expect("STORMCLOUD_TEST_SSH");
        let (user, address) = target.split_once('@').expect("user@host:port");
        let (host, port) = address.rsplit_once(':').map_or((address, DEFAULT_PORT), |(h, p)| (h, p.parse().unwrap()));
        let key = std::env::var("STORMCLOUD_TEST_SSH_KEY").ok();
        let password = std::env::var("STORMCLOUD_TEST_SSH_PASSWORD").ok();
        let server = Server {
            id: "test".into(),
            name: host.into(),
            host: host.into(),
            port,
            user: user.into(),
            auth: if key.is_some() { AuthMethod::Key } else { AuthMethod::Password },
            key_path: key.unwrap_or_default(),
        };
        let dir = std::env::temp_dir().join(format!("stormcloud-real-{}", new_id()));
        let file = dir.join("known_hosts");
        let first = check_with(&server, password.clone(), vec![file.clone()]).unwrap_err();
        let UnknownHostKey(host_key) = first.downcast_ref::<UnknownHostKey>().expect("an unknown host key first");
        trust_host_key(&server, host_key, &file).unwrap();
        let report = check_with(&server, password, vec![file]).unwrap();
        println!("{report:#?}");
        assert!(!report.kernel.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn known_host_key_types_are_asked_for_first() {
        let dir = std::env::temp_dir().join(format!("stormcloud-preferred-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("known_hosts");
        let files = vec![file.clone()];
        assert_eq!(preferred_host_keys("web", 22, &files), Preferred::DEFAULT.key.to_vec());
        std::fs::write(&file, "web ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQDnJlkoZ3Nq+9Gm1pM8hwuOkCq0VdQkLzxPSu0IPLLHZ6Vq7gW3U0dCtFaKD4Pl6E8Ah2N59Sx4qfAVwLqN37NQnBQ0ChVQtHAm4C4eyBG+iX+WvRZ/wMjtmGeXiprD0Fsv0Xx4+QHAhrSAvdbpU9Ftfsbr6cJbFzRlpbCyDqh+lw==\n").unwrap();
        let order = preferred_host_keys("web", 22, &files);
        assert!(matches!(order[0], Algorithm::Rsa { .. }));
        assert_eq!(order.len(), Preferred::DEFAULT.key.len());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_restart_is_not_a_no() {
        let r = parse_report("@@uname\nLinux\n6.1\nx86_64\n@@manager\napt\n@@reboot\nunknown\n@@end\n");
        assert_eq!(r.reboot_required, None);
    }

    #[test]
    fn expands_the_home_folder() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/.ssh/id_rsa"), home.join(".ssh").join("id_rsa"));
        assert_eq!(expand_home("/etc/key"), PathBuf::from("/etc/key"));
    }
}
