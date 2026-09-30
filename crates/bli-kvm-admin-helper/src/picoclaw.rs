//! PicoClaw configuration and chat history, as the runtime's own user.
//!
//! Runs as `hardened-blikvm-picoclaw` without network access and touches
//! only that user's home. It keeps the reference's configuration edits
//! (defaults, model entry, agent profile, gateway token, KVM MCP server) and
//! owns `.security.yml`, which it renders from its own 0600 store so that the
//! model API key is written for the runtime but never read back out. Chat
//! sessions are listed, read and deleted with bounded sizes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use bli_kvm_admin_helper::{
    INTERNAL_TOKEN_HEADER, PICOCLAW_GATEWAY_PORT, PicoclawFacts, PicoclawMessage,
    PicoclawSessionDetail, PicoclawSessionItem, valid_picoclaw_session_id,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Packaged agent profiles (`AGENT.md`, `AGENT_KVM.md`).
pub const PROFILE_DIRECTORY: &str = "/usr/share/hardened-blikvm/picoclaw";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SESSION_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SESSION_FILES: usize = 1000;
const MAX_DETAIL_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_CHARS: usize = 60;
const DEFAULT_SESSION_LIMIT: u32 = 20;
const ONBOARD_TIMEOUT: Duration = Duration::from_secs(60);
const PICO_SESSION_PREFIX: &str = "agent:main:pico:direct:pico:";
const OPAQUE_SESSION_PREFIX: &str = "sk_v1_";
const MCP_SERVER_NAME: &str = "blikvm";
const SECURITY_HEADER: &str = "# Managed by Hardened BliKVM; changes here are replaced.\n";

/// Secrets the helper keeps for the runtime (model API keys, gateway token).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secrets {
    #[serde(default)]
    model_keys: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pico_token: String,
}

pub struct Picoclaw {
    /// The runtime user's home (`HOME`).
    state: PathBuf,
    /// `PICOCLAW_HOME`.
    home: PathBuf,
    binary: PathBuf,
    profiles: PathBuf,
}

fn regular_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(io::Error::other("unexpected file"));
    }
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temporary, path)
}

fn random_hex(bytes: usize) -> io::Result<String> {
    let mut buffer = vec![0_u8; bytes];
    File::open("/dev/urandom")?.read_exact(&mut buffer)?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn object<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    let map = value.as_object_mut().expect("callers pass objects");
    let entry = map.entry(key).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("just made an object")
}

fn set_path(root: &mut Value, path: &[&str], value: Value) {
    let (last, parents) = path.split_last().expect("paths are not empty");
    let mut current = root;
    for key in parents {
        object(current, key);
        current = current.get_mut(*key).expect("just inserted");
    }
    current
        .as_object_mut()
        .expect("objects all the way down")
        .insert((*last).to_string(), value);
}

/// The reference's startup defaults for a KVM-attached runtime.
fn apply_defaults(config: &mut Value) {
    let defaults: [(&[&str], Value); 16] = [
        (
            &["agents", "defaults", "restrict_to_workspace"],
            json!(false),
        ),
        (
            &["agents", "defaults", "allow_read_outside_workspace"],
            json!(true),
        ),
        (
            &["agents", "defaults", "tool_feedback", "enabled"],
            json!(true),
        ),
        (&["gateway", "host"], json!("127.0.0.1")),
        (&["gateway", "port"], json!(PICOCLAW_GATEWAY_PORT)),
        (&["gateway", "hot_reload"], json!(false)),
        (&["tools", "cron", "allow_command"], json!(true)),
        (&["tools", "exec", "allow_remote"], json!(true)),
        (&["tools", "exec", "enable_deny_patterns"], json!(false)),
        (&["channel_list", "pico", "type"], json!("pico")),
        (&["channel_list", "pico", "enabled"], json!(true)),
        (
            &["channel_list", "pico", "settings", "allow_token_query"],
            json!(false),
        ),
        (
            &["channel_list", "pico", "settings", "ping_interval"],
            json!(30),
        ),
        (
            &["channel_list", "pico", "settings", "read_timeout"],
            json!(60),
        ),
        (
            &["channel_list", "pico", "settings", "write_timeout"],
            json!(10),
        ),
        (&["tools", "mcp", "enabled"], json!(true)),
    ];
    for (path, value) in defaults {
        set_path(config, path, value);
    }
    set_path(
        config,
        &["channel_list", "pico", "settings", "max_connections"],
        json!(100),
    );
}

/// The last segment of a provider-qualified model identifier.
pub fn model_name(model: &str) -> String {
    let model = model.trim();
    match model.rsplit_once('/') {
        Some((_, name)) if !name.trim().is_empty() => name.trim().to_string(),
        _ => model.to_string(),
    }
}

fn text(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        match current.get(*key) {
            Some(next) => current = next,
            None => return String::new(),
        }
    }
    current.as_str().unwrap_or_default().trim().to_string()
}

fn number(value: &Value, path: &[&str]) -> u64 {
    let mut current = value;
    for key in path {
        match current.get(*key) {
            Some(next) => current = next,
            None => return 0,
        }
    }
    current.as_u64().unwrap_or(0)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= limit {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(limit).collect::<String>())
    }
}

fn rfc3339(time: SystemTime) -> String {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

struct StoredSession {
    messages: Vec<PicoclawMessage>,
    summary: String,
    created: String,
    updated: String,
}

impl StoredSession {
    fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.summary.trim().is_empty()
    }

    fn visible(&self) -> impl Iterator<Item = &PicoclawMessage> {
        self.messages.iter().filter(|message| {
            matches!(message.role.as_str(), "user" | "assistant")
                && !message.content.trim().is_empty()
        })
    }
}

#[derive(Deserialize)]
struct StoredMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Value,
}

#[derive(Deserialize, Default)]
struct SessionMeta {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    skip: usize,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
}

#[derive(Deserialize)]
struct LegacySession {
    #[serde(default)]
    key: String,
    #[serde(default)]
    messages: Vec<StoredMessage>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    created: String,
    #[serde(default)]
    updated: String,
}

fn stored_message(message: StoredMessage) -> PicoclawMessage {
    PicoclawMessage {
        role: message.role,
        content: match message.content {
            Value::String(text) => text,
            _ => String::new(),
        },
    }
}

impl Picoclaw {
    pub fn new(
        state: impl Into<PathBuf>,
        binary: impl Into<PathBuf>,
        profiles: impl Into<PathBuf>,
    ) -> Self {
        let state = state.into();
        Self {
            home: state.join(".picoclaw"),
            state,
            binary: binary.into(),
            profiles: profiles.into(),
        }
    }

    fn config_path(&self) -> PathBuf {
        self.home.join("config.json")
    }

    fn secrets_path(&self) -> PathBuf {
        self.home.join("kvm-secrets.json")
    }

    fn load_config(&self) -> io::Result<Option<Value>> {
        match regular_file(&self.config_path(), MAX_CONFIG_BYTES) {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| io::Error::other("the PicoClaw configuration is not JSON"))?;
                if value.is_object() {
                    Ok(Some(value))
                } else {
                    Err(io::Error::other(
                        "the PicoClaw configuration is not an object",
                    ))
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn save_config(&self, config: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
        write_private(&self.config_path(), &bytes)
    }

    fn load_secrets(&self) -> io::Result<Secrets> {
        match regular_file(&self.secrets_path(), 64 * 1024) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| io::Error::other("the PicoClaw secret store is invalid")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Secrets::default()),
            Err(error) => Err(error),
        }
    }

    /// Stores the secrets and renders `.security.yml` from them; every
    /// scalar is a JSON string, which is a valid YAML double-quoted scalar.
    fn save_secrets(&self, secrets: &Secrets) -> io::Result<()> {
        let bytes = serde_json::to_vec(secrets).map_err(io::Error::other)?;
        write_private(&self.secrets_path(), &bytes)?;
        let quote = |value: &str| serde_json::to_string(value).map_err(io::Error::other);
        let mut yaml = String::from(SECURITY_HEADER);
        if secrets.model_keys.is_empty() {
            yaml.push_str("model_list: {}\n");
        } else {
            yaml.push_str("model_list:\n");
            for (name, keys) in &secrets.model_keys {
                yaml.push_str(&format!("  {}:\n    api_keys:\n", quote(name)?));
                for key in keys {
                    yaml.push_str(&format!("      - {}\n", quote(key)?));
                }
            }
        }
        yaml.push_str(&format!(
            "channel_list:\n  pico:\n    settings:\n      token: {}\n",
            quote(&secrets.pico_token)?
        ));
        write_private(&self.home.join(".security.yml"), yaml.as_bytes())
    }

    fn workspace(&self, config: Option<&Value>) -> PathBuf {
        let configured = config
            .map(|config| text(config, &["agents", "defaults", "workspace"]))
            .unwrap_or_default();
        match configured.as_str() {
            "" => self.home.join("workspace"),
            "~" => self.state.clone(),
            path => match path.strip_prefix("~/") {
                Some(rest) => self.state.join(rest),
                None => PathBuf::from(path),
            },
        }
    }

    pub fn facts(&self) -> io::Result<PicoclawFacts> {
        let Some(config) = self.load_config()? else {
            return Ok(PicoclawFacts::default());
        };
        let secrets = self.load_secrets()?;
        let target = text(&config, &["agents", "defaults", "model_name"]);
        let configured = !target.is_empty()
            && config
                .get("model_list")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|entry| text(entry, &["model_name"]) == target)
                .is_some_and(|entry| {
                    let prefix = format!("{target}:");
                    !text(entry, &["api_base"]).is_empty()
                        && (secrets.model_keys.iter().any(|(name, keys)| {
                            (name == &target || name.starts_with(&prefix)) && !keys.is_empty()
                        }) || !text(entry, &["api_key"]).is_empty()
                            || entry
                                .get("api_keys")
                                .and_then(Value::as_array)
                                .is_some_and(|keys| !keys.is_empty()))
                });
        let settings = ["channel_list", "pico", "settings"];
        let token = if secrets.pico_token.is_empty() {
            text(&config, &[settings[0], settings[1], settings[2], "token"])
        } else {
            secrets.pico_token.clone()
        };
        let profile_text = regular_file(
            &self.workspace(Some(&config)).join("AGENT.md"),
            MAX_CONFIG_BYTES,
        )
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
        let profile_name = profile_text
            .lines()
            .find_map(|line| line.trim().strip_prefix("name:").map(str::trim))
            .unwrap_or_default();
        Ok(PicoclawFacts {
            config_present: true,
            model_configured: configured,
            model_name: target,
            gateway_token: token,
            agent_profile: if profile_name == "pico" {
                "default".into()
            } else {
                "kvm".into()
            },
            ping_interval_ms: (number(
                &config,
                &[settings[0], settings[1], settings[2], "ping_interval"],
            ) * 1000)
                .min(u64::from(u32::MAX)) as u32,
            read_timeout_ms: (number(
                &config,
                &[settings[0], settings[1], settings[2], "read_timeout"],
            ) * 1000)
                .min(u64::from(u32::MAX)) as u32,
        })
    }

    /// `picoclaw onboard` creates the default configuration and workspace.
    fn onboard(&self) -> io::Result<()> {
        fs::create_dir_all(&self.home)?;
        fs::set_permissions(&self.home, fs::Permissions::from_mode(0o700))?;
        let mut child = Command::new(&self.binary)
            .arg("onboard")
            .env_clear()
            .env("HOME", &self.state)
            .env("PICOCLAW_HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(io::Error::other("picoclaw onboard failed"));
                }
                break;
            }
            if started.elapsed() > ONBOARD_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("picoclaw onboard timed out"));
            }
            thread::sleep(Duration::from_millis(100));
        }
        if !self.config_path().is_file() {
            return Err(io::Error::other("picoclaw onboard wrote no configuration"));
        }
        self.write_profile("kvm")
    }

    fn write_profile(&self, profile: &str) -> io::Result<()> {
        let source = match profile {
            "default" => "AGENT.md",
            "kvm" => "AGENT_KVM.md",
            _ => return Err(io::Error::other("invalid agent profile")),
        };
        let content = regular_file(&self.profiles.join(source), MAX_CONFIG_BYTES)?;
        let workspace = self.workspace(self.load_config()?.as_ref());
        fs::create_dir_all(&workspace)?;
        let target = workspace.join("AGENT.md");
        let temporary = workspace.join(".AGENT.md.new");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&content)?;
        drop(file);
        fs::rename(temporary, target)
    }

    pub fn profile(&self, profile: &str) -> io::Result<()> {
        if self.load_config()?.is_none() {
            return Err(io::Error::other("picoclaw is not configured"));
        }
        self.write_profile(profile)
    }

    pub fn prepare(&self, internal_token: &str, mcp_url: &str) -> io::Result<()> {
        if internal_token.len() != 64 || !internal_token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::Error::other("invalid internal token"));
        }
        if !mcp_url.starts_with("http://127.0.0.1:") || mcp_url.len() > 128 {
            return Err(io::Error::other("invalid MCP address"));
        }
        let mut config = match self.load_config()? {
            Some(config) => config,
            None => {
                self.onboard()?;
                self.load_config()?
                    .ok_or_else(|| io::Error::other("picoclaw onboard wrote no configuration"))?
            }
        };
        let before = config.clone();
        apply_defaults(&mut config);
        set_path(
            &mut config,
            &["tools", "mcp", "servers", MCP_SERVER_NAME],
            json!({
                "enabled": true,
                "type": "http",
                "url": mcp_url,
                "headers": { INTERNAL_TOKEN_HEADER: internal_token },
            }),
        );
        // The token lives in `.security.yml` only.
        if let Some(settings) = config
            .pointer_mut("/channel_list/pico/settings")
            .and_then(Value::as_object_mut)
        {
            settings.remove("token");
        }
        if config != before {
            self.save_config(&config)?;
        }
        let mut secrets = self.load_secrets()?;
        if secrets.pico_token.is_empty() {
            secrets.pico_token = random_hex(24)?;
        }
        self.save_secrets(&secrets)
    }

    pub fn model(&self, model: &str, api_base: &str, api_key: &str) -> io::Result<String> {
        let (model, api_base, api_key) = (model.trim(), api_base.trim(), api_key.trim());
        let printable = |value: &str| value.chars().all(|ch| !ch.is_control());
        if api_base.is_empty() || api_base.len() > 512 || !printable(api_base) {
            return Err(io::Error::other("model api_base is required"));
        }
        if !(api_base.starts_with("https://") || api_base.starts_with("http://")) {
            return Err(io::Error::other("model api_base must be an http(s) URL"));
        }
        if api_key.is_empty()
            || api_key.len() > 1024
            || api_key
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(io::Error::other("model api_key is required"));
        }
        if model.is_empty() || model.len() > 128 || !printable(model) {
            return Err(io::Error::other("model identifier is required"));
        }
        let name = model_name(model);
        let mut config = self
            .load_config()?
            .ok_or_else(|| io::Error::other("picoclaw is not configured"))?;
        let root = config.as_object_mut().expect("configs are objects");
        let list = root.entry("model_list").or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        let list = list.as_array_mut().expect("just made an array");
        let index = match list
            .iter()
            .position(|entry| text(entry, &["model_name"]) == name)
        {
            Some(index) => {
                let entry = list[index].as_object_mut();
                if let Some(entry) = entry {
                    entry.insert("model_name".into(), json!(name));
                    entry.insert("model".into(), json!(model));
                    entry.insert("api_base".into(), json!(api_base));
                    entry.remove("api_key");
                    entry.remove("api_keys");
                } else {
                    list[index] = json!({"model_name": name, "model": model, "api_base": api_base});
                }
                index
            }
            None => {
                list.push(json!({"model_name": name, "model": model, "api_base": api_base}));
                list.len() - 1
            }
        };
        // The reference keys the secret by the entry's position among the
        // entries with the same name.
        let ordinal = list[..index]
            .iter()
            .filter(|entry| text(entry, &["model_name"]) == name)
            .count();
        set_path(
            &mut config,
            &["agents", "defaults", "model_name"],
            json!(name),
        );
        if let Some(defaults) = config
            .pointer_mut("/agents/defaults")
            .and_then(Value::as_object_mut)
        {
            defaults.remove("model");
        }
        self.save_config(&config)?;
        let mut secrets = self.load_secrets()?;
        secrets
            .model_keys
            .retain(|key, _| key != &name && !key.starts_with(&format!("{name}:")));
        secrets
            .model_keys
            .insert(format!("{name}:{ordinal}"), vec![api_key.to_string()]);
        self.save_secrets(&secrets)?;
        Ok(name)
    }

    fn sessions_dir(&self) -> io::Result<PathBuf> {
        Ok(self
            .workspace(self.load_config()?.as_ref())
            .join("sessions"))
    }

    fn read_jsonl(&self, path: &Path, skip: usize) -> io::Result<Vec<PicoclawMessage>> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_SESSION_FILE_BYTES {
            return Err(io::Error::other("unexpected session file"));
        }
        let mut messages = Vec::new();
        for (index, line) in BufReader::new(file.take(MAX_SESSION_FILE_BYTES))
            .lines()
            .enumerate()
        {
            let line = line?;
            if line.trim().is_empty() || index < skip {
                continue;
            }
            if let Ok(message) = serde_json::from_str::<StoredMessage>(&line) {
                messages.push(stored_message(message));
            }
        }
        Ok(messages)
    }

    fn modified(path: &Path) -> String {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .map(rfc3339)
            .unwrap_or_default()
    }

    /// A session stored as `<base>.jsonl` plus an optional `<base>.meta.json`.
    fn read_jsonl_session(&self, dir: &Path, base: &str) -> io::Result<StoredSession> {
        let jsonl = dir.join(format!("{base}.jsonl"));
        let meta = match regular_file(&dir.join(format!("{base}.meta.json")), MAX_CONFIG_BYTES) {
            Ok(bytes) => serde_json::from_slice::<SessionMeta>(&bytes).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => SessionMeta::default(),
            Err(error) => return Err(error),
        };
        let messages = self.read_jsonl(&jsonl, meta.skip)?;
        let fallback = Self::modified(&jsonl);
        Ok(StoredSession {
            messages,
            summary: meta.summary,
            created: if meta.created_at.is_empty() {
                fallback.clone()
            } else {
                meta.created_at
            },
            updated: if meta.updated_at.is_empty() {
                fallback
            } else {
                meta.updated_at
            },
        })
    }

    fn read_legacy_session(&self, path: &Path) -> io::Result<(String, StoredSession)> {
        let session: LegacySession =
            serde_json::from_slice(&regular_file(path, MAX_SESSION_FILE_BYTES)?)
                .map_err(|_| io::Error::other("invalid session file"))?;
        Ok((
            session.key,
            StoredSession {
                messages: session.messages.into_iter().map(stored_message).collect(),
                summary: session.summary,
                created: session.created,
                updated: session.updated,
            },
        ))
    }

    fn base_for(id: &str) -> String {
        if id.starts_with(OPAQUE_SESSION_PREFIX) {
            id.to_string()
        } else {
            format!("{PICO_SESSION_PREFIX}{id}").replace(':', "_")
        }
    }

    fn load_session(&self, dir: &Path, id: &str) -> io::Result<StoredSession> {
        let base = Self::base_for(id);
        match self.read_jsonl_session(dir, &base) {
            Ok(session) if !session.is_empty() => return Ok(session),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if !id.starts_with(OPAQUE_SESSION_PREFIX) {
            let (_, session) = self.read_legacy_session(&dir.join(format!("{base}.json")))?;
            if !session.is_empty() {
                return Ok(session);
            }
        }
        Err(io::ErrorKind::NotFound.into())
    }

    pub fn sessions(&self, offset: u32, limit: u32) -> io::Result<Vec<PicoclawSessionItem>> {
        let dir = self.sessions_dir()?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let sanitized_prefix = PICO_SESSION_PREFIX.replace(':', "_");
        let mut items = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries.take(MAX_SESSION_FILES) {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = if let Some(base) = name.strip_suffix(".jsonl") {
                if base.starts_with(OPAQUE_SESSION_PREFIX) {
                    base.to_string()
                } else if let Some(id) = base.strip_prefix(&sanitized_prefix) {
                    id.to_string()
                } else {
                    continue;
                }
            } else if name.ends_with(".json") && !name.ends_with(".meta.json") {
                match self.read_legacy_session(&entry.path()) {
                    Ok((key, _)) => match key.strip_prefix(PICO_SESSION_PREFIX) {
                        Some(id) => id.to_string(),
                        None => continue,
                    },
                    Err(_) => continue,
                }
            } else {
                continue;
            };
            if !valid_picoclaw_session_id(&id) || seen.contains(&id) {
                continue;
            }
            let Ok(session) = self.load_session(&dir, &id) else {
                continue;
            };
            seen.insert(id.clone());
            let preview = session
                .messages
                .iter()
                .find(|message| message.role == "user" && !message.content.trim().is_empty())
                .map(|message| message.content.clone())
                .unwrap_or_default();
            let mut title = truncate_chars(&session.summary, PREVIEW_CHARS);
            if title.is_empty() {
                title = truncate_chars(&preview, PREVIEW_CHARS);
            }
            let mut preview = truncate_chars(&preview, PREVIEW_CHARS);
            if preview.is_empty() {
                preview = "(empty)".into();
            }
            if title.is_empty() {
                title.clone_from(&preview);
            }
            items.push(PicoclawSessionItem {
                id,
                title,
                preview,
                message_count: session.visible().count() as u32,
                created: session.created,
                updated: session.updated,
            });
        }
        items.sort_by(|left, right| right.updated.cmp(&left.updated));
        let limit = if limit == 0 {
            DEFAULT_SESSION_LIMIT
        } else {
            limit.min(100)
        } as usize;
        Ok(items
            .into_iter()
            .skip(offset as usize)
            .take(limit)
            .collect())
    }

    pub fn session(&self, id: &str) -> io::Result<PicoclawSessionDetail> {
        if !valid_picoclaw_session_id(id) {
            return Err(io::Error::other("invalid session id"));
        }
        let session = self
            .load_session(&self.sessions_dir()?, id)
            .map_err(|_| io::Error::other("session not found"))?;
        let mut total = 0;
        let mut messages = Vec::new();
        for message in session.visible() {
            total += message.content.len();
            if total > MAX_DETAIL_BYTES {
                break;
            }
            messages.push(message.clone());
        }
        Ok(PicoclawSessionDetail {
            id: id.to_string(),
            messages,
            summary: session.summary,
            created: session.created,
            updated: session.updated,
        })
    }

    pub fn delete_session(&self, id: &str) -> io::Result<()> {
        if !valid_picoclaw_session_id(id) {
            return Err(io::Error::other("invalid session id"));
        }
        let dir = self.sessions_dir()?;
        let base = Self::base_for(id);
        let mut removed = false;
        for suffix in [".jsonl", ".meta.json", ".json"] {
            match fs::remove_file(dir.join(format!("{base}{suffix}"))) {
                Ok(()) => removed = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if removed {
            Ok(())
        } else {
            Err(io::Error::other("session not found"))
        }
    }

    /// Removes the runtime's configuration, secrets and history.
    pub fn reset(&self) -> io::Result<()> {
        match fs::remove_dir_all(&self.home) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Picoclaw) {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        fs::create_dir(&profiles).unwrap();
        fs::write(profiles.join("AGENT.md"), "---\nname: pico\n---\n").unwrap();
        fs::write(profiles.join("AGENT_KVM.md"), "---\nname: pico-kvm\n---\n").unwrap();
        // A stand-in for `picoclaw onboard` that writes a minimal config.
        let binary = dir.path().join("picoclaw");
        fs::write(
            &binary,
            "#!/bin/sh\nmkdir -p \"$PICOCLAW_HOME/workspace\"\nprintf '{\"agents\":{\"defaults\":{\"model\":\"x\"}},\"model_list\":[]}' > \"$PICOCLAW_HOME/config.json\"\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let picoclaw = Picoclaw::new(dir.path().join("state"), binary, profiles);
        fs::create_dir(dir.path().join("state")).unwrap();
        (dir, picoclaw)
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn prepare_onboards_applies_defaults_and_keeps_secrets_out_of_the_config() {
        let (_dir, picoclaw) = setup();
        assert_eq!(picoclaw.facts().unwrap(), PicoclawFacts::default());
        picoclaw
            .prepare(TOKEN, "http://127.0.0.1:18080/internal/picoclaw/mcp")
            .unwrap();
        let config = picoclaw.load_config().unwrap().unwrap();
        assert_eq!(config.pointer("/gateway/port"), Some(&json!(18790)));
        assert_eq!(
            config.pointer("/tools/mcp/servers/blikvm/headers/X-BliKVM-Internal-Token"),
            Some(&json!(TOKEN))
        );
        assert_eq!(
            config.pointer("/channel_list/pico/enabled"),
            Some(&json!(true))
        );
        let facts = picoclaw.facts().unwrap();
        assert!(facts.config_present && !facts.model_configured);
        assert_eq!(facts.gateway_token.len(), 48);
        assert_eq!(facts.agent_profile, "kvm");
        assert_eq!(facts.ping_interval_ms, 30_000);
        assert!(picoclaw.prepare("short", "http://127.0.0.1:1/").is_err());
        assert!(picoclaw.prepare(TOKEN, "http://example.org/").is_err());

        let name = picoclaw
            .model("openai/gpt-4o-mini", "https://api.example/v1", "sk-secret")
            .unwrap();
        assert_eq!(name, "gpt-4o-mini");
        let facts = picoclaw.facts().unwrap();
        assert!(facts.model_configured);
        assert_eq!(facts.model_name, "gpt-4o-mini");
        let config = fs::read_to_string(picoclaw.config_path()).unwrap();
        assert!(!config.contains("sk-secret"), "{config}");
        let security = fs::read_to_string(picoclaw.home.join(".security.yml")).unwrap();
        assert!(security.contains("\"gpt-4o-mini:0\":"), "{security}");
        assert!(security.contains("- \"sk-secret\""), "{security}");
        assert!(security.contains(&format!("token: \"{}\"", facts.gateway_token)));
        let mode = fs::metadata(picoclaw.home.join(".security.yml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(picoclaw.model("m", "ftp://x", "k").is_err());
        assert!(picoclaw.model("m", "https://x", "a b").is_err());

        picoclaw.profile("default").unwrap();
        assert_eq!(picoclaw.facts().unwrap().agent_profile, "default");
        assert!(picoclaw.profile("other").is_err());
        picoclaw.reset().unwrap();
        assert_eq!(picoclaw.facts().unwrap(), PicoclawFacts::default());
    }

    #[test]
    fn sessions_are_listed_read_and_deleted_as_the_reference_stores_them() {
        let (_dir, picoclaw) = setup();
        picoclaw
            .prepare(TOKEN, "http://127.0.0.1:18080/internal/picoclaw/mcp")
            .unwrap();
        let sessions = picoclaw.home.join("workspace/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let base = "agent_main_pico_direct_pico_3f2a9c1e-5b7d";
        fs::write(
            sessions.join(format!("{base}.jsonl")),
            "{\"role\":\"user\",\"content\":\"open notepad please\"}\n{\"role\":\"tool\",\"content\":\"x\"}\n{\"role\":\"assistant\",\"content\":\"done\"}\n",
        )
        .unwrap();
        fs::write(
            sessions.join(format!("{base}.meta.json")),
            "{\"summary\":\"Notepad\",\"skip\":0,\"updated_at\":\"2026-09-26T10:00:00Z\"}",
        )
        .unwrap();
        fs::write(sessions.join("unrelated.jsonl"), "{}\n").unwrap();
        let items = picoclaw.sessions(0, 0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "3f2a9c1e-5b7d");
        assert_eq!(items[0].title, "Notepad");
        assert_eq!(items[0].preview, "open notepad please");
        assert_eq!(items[0].message_count, 2);
        let detail = picoclaw.session("3f2a9c1e-5b7d").unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert!(picoclaw.session("../etc").is_err());
        picoclaw.delete_session("3f2a9c1e-5b7d").unwrap();
        assert!(picoclaw.delete_session("3f2a9c1e-5b7d").is_err());
        assert!(picoclaw.sessions(0, 0).unwrap().is_empty());
    }

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)),
            "2026-09-21T14:13:20Z"
        );
    }
}
