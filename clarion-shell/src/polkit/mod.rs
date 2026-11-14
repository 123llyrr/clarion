// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure polkit agent logic: wire strings, cookies, identities and the helper
//! PAM conversation. Nothing here touches D-Bus, the UI or a real credential:
//! those live in `services::polkit`, `view::polkit` and `polkit::helper`.
pub mod helper;

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

pub const AUTHORITY_NAME: &str = "org.freedesktop.PolicyKit1";
pub const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
pub const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
pub const AGENT_PATH: &str = "/org/clarion/PolicyKit1/AuthenticationAgent";

/// One user the authority is willing to authenticate. P1 only understands
/// `unix-user`; groups and other identity kinds are rejected by the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthIdentity {
    pub uid: u32,
    pub username: String,
}

/// A pending `BeginAuthentication` call, reduced to what the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthRequest {
    pub cookie: String,
    pub action_id: String,
    pub message: String,
    /// The action's icon name, when it supplied one.
    pub icon: Option<String>,
    /// The application that initiated the request, when polkit exposed its
    /// pid. `name` is a human label; `icon` is a theme name or absolute path.
    pub app_name: Option<String>,
    pub app_icon: Option<String>,
    pub details: Vec<(String, String)>,
    pub identity: AuthIdentity,
    /// How many further requests are waiting behind this one.
    pub queued: usize,
}

/// Whether the agent is registered, and why not when it is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthStatus {
    pub registered: bool,
    pub reason: Option<String>,
}

impl AuthStatus {
    pub fn ready() -> Self {
        Self {
            registered: true,
            reason: None,
        }
    }
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            registered: false,
            reason: Some(reason.into()),
        }
    }
}

/// A valid cookie is a short single line. Newlines would desynchronise the
/// helper's line-oriented protocol, and an empty cookie can never match.
pub fn valid_cookie(cookie: &str) -> bool {
    !cookie.is_empty()
        && cookie.len() <= 4096
        && !cookie
            .chars()
            .any(|c| c == '\n' || c == '\r' || c == '\0')
}

/// Decode the GLib `g_strescape` form the helper prints. Non-ASCII bytes are
/// octal escaped by GLib, so decoding works on bytes and validates UTF-8 last.
pub fn decode_strescape(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            out.push(b'\\');
            break;
        }
        match bytes[i + 1] {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'v' => out.push(0x0b),
            b'\\' => out.push(b'\\'),
            b'"' => out.push(b'"'),
            b'\'' => out.push(b'\''),
            b'0'..=b'7' => {
                let mut value: u32 = 0;
                let mut digits = 0;
                let mut j = i + 1;
                while j < bytes.len() && digits < 3 && matches!(bytes[j], b'0'..=b'7') {
                    value = value * 8 + u32::from(bytes[j] - b'0');
                    digits += 1;
                    j += 1;
                }
                // A byte value above 255 is not an escape we can honour.
                if value <= 0xff {
                    out.push(value as u8);
                } else {
                    out.push(b'\\');
                    out.extend_from_slice(&bytes[i + 1..j]);
                }
                i = j;
                continue;
            }
            // GLib leaves an unknown escape as the backslash itself.
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One line of the helper's PAM conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperEvent {
    /// A PAM prompt. `echo` distinguishes `PAM_PROMPT_ECHO_ON` (username) from
    /// the masked password prompt.
    Prompt { text: String, echo: bool },
    Info(String),
    Error(String),
    Success,
    Failure,
    /// The helper closed without a terminal line.
    Closed,
}

/// Parse one helper line. Unknown lines are surfaced as `Info` so a newer
/// helper cannot silently stall the dialog.
pub fn parse_helper_line(line: &str) -> Option<HelperEvent> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    if line == "SUCCESS" {
        return Some(HelperEvent::Success);
    }
    if line == "FAILURE" {
        return Some(HelperEvent::Failure);
    }
    let (tag, body) = line.split_once(' ').unwrap_or((line, ""));
    Some(match tag {
        "PAM_PROMPT_ECHO_OFF" => HelperEvent::Prompt {
            text: decode_strescape(body),
            echo: false,
        },
        "PAM_PROMPT_ECHO_ON" => HelperEvent::Prompt {
            text: decode_strescape(body),
            echo: true,
        },
        "PAM_TEXT_INFO" => HelperEvent::Info(decode_strescape(body)),
        "PAM_ERROR_MSG" => HelperEvent::Error(decode_strescape(body)),
        _ => HelperEvent::Info(decode_strescape(line)),
    })
}

/// Resolve a uid through the C library. Reading `/etc/passwd` by hand would
/// miss NSS (LDAP, systemd-homed, sssd).
pub fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0i8; 4096];
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || passwd.pw_name.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(passwd.pw_name) };
    name.to_str().ok().map(str::to_owned)
}

/// The application that initiated an authentication request, resolved from
/// the process polkit is asking about. The dialog uses its name and icon
/// instead of the action's generic artwork.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequesterApp {
    pub name: Option<String>,
    pub icon: Option<String>,
}

/// Resolve the requester from the request details. Polkitd adds both the
/// subject and caller pids; the subject is the process the user is acting
/// through (pkexec from a terminal, or the GUI app), so prefer it.
pub fn requester_app(details: &HashMap<String, String>) -> Option<RequesterApp> {
    for key in ["polkit.subject-pid", "polkit.caller-pid"] {
        let Some(pid) = details.get(key).and_then(|pid| pid.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(app) = walk_to_app(pid) {
            return Some(app);
        }
    }
    None
}

/// Privilege/launcher processes have no useful icon of their own: the person
/// wants to see the shell or application that invoked them.
const LAUNCHERS: &[&str] = &[
    "pkexec",
    "sudo",
    "doas",
    "systemctl",
    "busctl",
    "gdbus",
    "dbus-send",
    "python",
    "python3",
    "node",
    "perl",
    "ruby",
    "env",
];

fn walk_to_app(pid: u32) -> Option<RequesterApp> {
    let mut pid = pid;
    for _ in 0..4 {
        if let Some(app) = app_for_pid(pid) {
            return Some(app);
        }
        pid = parent_pid(pid)?;
    }
    None
}

fn app_for_pid(pid: u32) -> Option<RequesterApp> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let base = exe.file_name()?.to_str()?.trim();
    if base.is_empty() || LAUNCHERS.contains(&base) {
        return None;
    }
    // A terminal is a better description than the shell binary's name, and it
    // gives the dialog an icon the theme actually carries.
    if matches!(
        base,
        "zsh" | "bash" | "sh" | "fish" | "dash" | "ksh" | "nu" | "elvish" | "xonsh"
    ) {
        return Some(RequesterApp {
            name: Some("终端".into()),
            icon: Some("utilities-terminal".into()),
        });
    }
    match desktop_entry_for(base) {
        Some((name, icon)) => Some(RequesterApp { name, icon }),
        None => Some(RequesterApp {
            name: None,
            icon: Some(base.to_owned()),
        }),
    }
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field can contain spaces and parentheses; the fields after the
    // closing parenthesis start with state, then ppid.
    let rest = stat.rsplit_once(") ")?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// The details a person can use. Polkitd injects process bookkeeping such as
/// `polkit.caller-pid`; those belong in the journal, not the dialog.
pub fn visible_details(details: &[(String, String)]) -> Vec<(&str, &str)> {
    details
        .iter()
        .filter(|(key, value)| {
            !key.is_empty() && !value.is_empty() && !key.starts_with("polkit.")
        })
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .take(2)
        .collect()
}

fn application_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(std::path::PathBuf::from(data).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(
            std::path::PathBuf::from(home)
                .join(".local/share/applications"),
        );
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| std::path::PathBuf::from(dir).join("applications")),
    );
    dirs
}

fn desktop_entry_for(exec_name: &str) -> Option<(Option<String>, Option<String>)> {
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(found) = match_desktop_entry(&text, exec_name) {
                return Some(found);
            }
        }
    }
    None
}

fn match_desktop_entry(
    text: &str,
    exec_name: &str,
) -> Option<(Option<String>, Option<String>)> {
    let mut name = None;
    let mut icon = None;
    let mut exec = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Name=") {
            if name.is_none() {
                name = Some(value.trim().to_owned());
            }
        } else if let Some(value) = line.strip_prefix("Icon=") {
            icon = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("Exec=") {
            exec = Some(value.trim().to_owned());
        } else if line == "NoDisplay=true" || line == "Hidden=true" {
            hidden = true;
        }
    }
    if hidden {
        return None;
    }
    if exec_program(&exec?)? != exec_name {
        return None;
    }
    Some((
        name.filter(|value| !value.is_empty()),
        icon.filter(|value| !value.is_empty()),
    ))
}

/// The program token of a desktop entry's `Exec=`. It may sit behind `env`
/// assignments, may be quoted, and is followed by field codes (`%U`).
fn exec_program(exec: &str) -> Option<String> {
    let tokens = exec_tokens(exec);
    let mut tokens = tokens.iter();
    let mut program = tokens.next()?;
    if program == "env" {
        program = tokens.find(|token| !token.contains('='))?;
    }
    let name = std::path::Path::new(program).file_name()?.to_str()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn exec_tokens(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in exec.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quote == Some('"') => escaped = true,
            '"' | '\'' if quote.is_none() => quote = Some(character),
            character if Some(character) == quote => quote = None,
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Pick the first `unix-user` identity from the authority's list.
pub fn pick_identity(identities: &[(String, HashMap<String, OwnedValue>)]) -> Option<AuthIdentity> {
    identities.iter().find_map(|(kind, details)| {
        if kind != "unix-user" {
            return None;
        }
        let uid = details.get("uid").and_then(|value| u32::try_from(value).ok())?;
        let username = user_name(uid)?;
        Some(AuthIdentity { uid, username })
    })
}

/// Build a `(sa{sv})` unix-session subject.
pub fn unix_session_subject(
    session_id: &str,
) -> (String, HashMap<String, OwnedValue>) {
    let mut details = HashMap::new();
    details.insert(
        "session-id".to_owned(),
        OwnedValue::try_from(zbus::zvariant::Value::from(session_id))
            .expect("string is always a valid variant"),
    );
    ("unix-session".to_owned(), details)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_glib_escapes() {
        assert_eq!(decode_strescape(r#"a\nb"#), "a\nb");
        assert_eq!(decode_strescape(r#"a\tb\\c\"d"#), "a\tb\\c\"d");
        assert_eq!(decode_strescape(r#"\141\142\143"#), "abc");
        // GLib escapes UTF-8 bytes individually; octal bytes reassemble.
        assert_eq!(decode_strescape(r#"\344\270\255\346\226\207"#), "中文");
        assert_eq!(decode_strescape(r#"bad\q"#), r#"bad\q"#);
    }

    #[test]
    fn parses_every_helper_line() {
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_OFF Password: \n"),
            Some(HelperEvent::Prompt {
                text: "Password: ".into(),
                echo: false,
            })
        );
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_ON Login: "),
            Some(HelperEvent::Prompt {
                text: "Login: ".into(),
                echo: true,
            })
        );
        assert_eq!(
            parse_helper_line("PAM_ERROR_MSG Authentication failure"),
            Some(HelperEvent::Error("Authentication failure".into()))
        );
        assert_eq!(
            parse_helper_line("PAM_TEXT_INFO Password expired"),
            Some(HelperEvent::Info("Password expired".into()))
        );
        assert_eq!(parse_helper_line("SUCCESS"), Some(HelperEvent::Success));
        assert_eq!(parse_helper_line("FAILURE"), Some(HelperEvent::Failure));
        assert_eq!(parse_helper_line(""), None);
    }

    #[test]
    fn validates_cookies() {
        assert!(valid_cookie("abc-123"));
        assert!(!valid_cookie(""));
        assert!(!valid_cookie("two\nlines"));
        assert!(!valid_cookie(&"x".repeat(4097)));
    }

    #[test]
    fn rejects_unsupported_identities() {
        let user = AuthIdentity {
            uid: 1000,
            username: "tester".into(),
        };
        assert_eq!(
            pick_identity(&[(
                "unix-user".into(),
                HashMap::from([(
                    "uid".into(),
                    OwnedValue::try_from(zbus::zvariant::Value::from(user.uid)).unwrap()
                )])
            )])
            .map(|identity| identity.uid),
            Some(1000)
        );
        assert!(pick_identity(&[("unix-group".into(), HashMap::new())]).is_none());
    }

    #[test]
    fn subject_uses_the_session_signature() {
        let (kind, details) = unix_session_subject("c2");
        assert_eq!(kind, "unix-session");
        assert_eq!(
            String::try_from(details.get("session-id").unwrap().clone()).unwrap(),
            "c2"
        );
    }

    #[test]
    fn resolves_desktop_exec_and_skips_hidden_entries() {
        assert_eq!(
            exec_program("env FOO=1 /usr/bin/gnome-disks %U").as_deref(),
            Some("gnome-disks")
        );
        assert_eq!(
            exec_program("\"/opt/My App/app\" --flag").as_deref(),
            Some("app")
        );
        assert_eq!(exec_program("app %f").as_deref(), Some("app"));

        let entry = "[Desktop Entry]\nName=Disks\nExec=gnome-disks %U\nIcon=org.gnome.DiskUtility\n";
        assert_eq!(
            match_desktop_entry(entry, "gnome-disks"),
            Some((
                Some("Disks".into()),
                Some("org.gnome.DiskUtility".into())
            ))
        );
        assert_eq!(match_desktop_entry(entry, "other"), None);

        let hidden = "[Desktop Entry]\nName=Hidden\nExec=foo\nHidden=true\n";
        assert_eq!(match_desktop_entry(hidden, "foo"), None);
    }

    #[test]
    fn hides_polkit_bookkeeping_details() {
        let details = vec![
            ("polkit.caller-pid".into(), "1234".into()),
            ("polkit.subject-pid".into(), "5678".into()),
        ];
        assert!(visible_details(&details).is_empty());

        let details = vec![
            ("polkit.caller-pid".into(), "1234".into()),
            ("user".into(), "tester".into()),
            ("reason".into(), "管理磁盘".into()),
            ("extra".into(), "ignored".into()),
        ];
        assert_eq!(
            visible_details(&details),
            vec![("user", "tester"), ("reason", "管理磁盘")]
        );
    }

    #[test]
    fn reads_a_process_parent_from_proc() {
        let parent = parent_pid(std::process::id()).expect("the test process has a parent");
        assert!(parent > 0);
    }
}
