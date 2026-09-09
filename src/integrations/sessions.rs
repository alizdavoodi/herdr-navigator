use std::{collections::HashSet, env, path::PathBuf, process::Command, sync::OnceLock};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    config::{Config, SessionEntryConfig},
    herdr::{herdr_bin, herdr_json},
    model::{Entry, EntryAction, Source},
    paths::home,
};

#[derive(Debug, Deserialize)]
struct SessionList {
    #[serde(default)]
    sessions: Vec<ListedSession>,
}

#[derive(Debug, Deserialize)]
struct ListedSession {
    name: String,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    default: bool,
    session_dir: Option<String>,
}

pub(crate) fn collect_sessions(config: &Config) -> Vec<Entry> {
    let mut entries = Vec::new();
    if config.sessions.local {
        entries.extend(collect_local_sessions());
    }
    entries.extend(
        config
            .sessions
            .entries
            .iter()
            .filter(|entry| entry.remote.is_none())
            .map(manual_session_entry),
    );
    entries
}

#[derive(Debug, Deserialize)]
struct ListedMachine {
    label: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    selected: bool,
}

fn listed_machines() -> Vec<ListedMachine> {
    let json = herdr_json(["machine", "list", "--json"]).unwrap_or(Value::Null);
    // Accept a bare `[...]` array or a `machines: [...]` envelope, mirroring
    // the session-list tolerance above.
    let parsed: Vec<ListedMachine> = serde_json::from_value(json.clone())
        .or_else(|_| {
            serde_json::from_value(json.pointer("/machines").cloned().unwrap_or(Value::Null))
        })
        .unwrap_or_default();
    parsed
}

pub(crate) fn collect_remotes(config: &Config) -> Vec<Entry> {
    let mut targets = HashSet::new();
    let mut entries = Vec::new();
    for machine in listed_machines() {
        let Some(entry) = machine_entry(&machine, &config.sessions.entries, &mut targets) else {
            continue;
        };
        entries.push(entry);
    }
    entries.extend(
        config
            .sessions
            .entries
            .iter()
            .filter(|config| {
                config
                    .remote
                    .as_deref()
                    .is_some_and(|target| !targets.contains(target))
            })
            .filter_map(remote_entry),
    );
    entries
}

fn machine_entry(
    machine: &ListedMachine,
    manual: &[SessionEntryConfig],
    targets: &mut HashSet<String>,
) -> Option<Entry> {
    let target = machine.target.clone()?;
    targets.insert(target.clone());
    let label = machine
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(&target)
        .to_string();

    // Tag this machine when an operator-managed `[[sessions]]` entry points
    // at the same target so it stays discoverable by that entry's tags.
    let mut search_terms: Vec<String> = vec!["server".into(), "remote".into(), "machine".into()];
    if let Some(entry) = manual
        .iter()
        .find(|entry| entry.remote.as_deref() == Some(target.as_str()))
    {
        search_terms.extend(entry.tags.iter().cloned());
    }
    search_terms.push(target.clone());
    search_terms.push(label.clone());

    let mut flags = Vec::new();
    if machine.selected {
        flags.push("connected");
    }
    if !machine.enabled {
        flags.push("disabled");
    }
    if let Some(session) = machine.session.as_deref().filter(|s| !s.is_empty()) {
        flags.push(session);
    }
    let subtitle = if flags.is_empty() {
        format!("ssh machine · {target}")
    } else {
        format!("ssh machine · {flags} · {target}", flags = flags.join(" · "))
    };

    Some(Entry {
        source: Source::Server,
        title: label,
        subtitle,
        path: PathBuf::from(format!("remote:{target}")),
        workspace_id: None,
        workspace_label: None,
        agent_target: None,
        project: None,
        action: EntryAction::OpenRemote {
            target: target.clone(),
        },
        source_label: None,
        search_terms,
        agent_kind: None,
        agent_task: None,
        canonical: OnceLock::new(),
    })
}

fn collect_local_sessions() -> Vec<Entry> {
    let json = herdr_json(["session", "list", "--json"]).unwrap_or(Value::Null);
    let list: SessionList = serde_json::from_value(json.clone())
        .or_else(|_| {
            serde_json::from_value(json.pointer("/result").cloned().unwrap_or(Value::Null))
        })
        .unwrap_or(SessionList { sessions: vec![] });
    list.sessions.into_iter().map(local_session_entry).collect()
}

fn local_session_entry(session: ListedSession) -> Entry {
    let mut flags = Vec::new();
    if session.default {
        flags.push("default");
    }
    if session.running {
        flags.push("running");
    }
    let subtitle = if flags.is_empty() {
        "local session".into()
    } else {
        format!("local session · {}", flags.join(" · "))
    };
    let path = session
        .session_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(format!(".config/herdr/sessions/{}", session.name)));
    Entry {
        source: Source::Session,
        title: session.name.clone(),
        subtitle,
        path,
        workspace_id: None,
        workspace_label: None,
        agent_target: None,
        project: None,
        action: EntryAction::AttachSession {
            name: session.name,
            remote: None,
        },
        source_label: None,
        search_terms: vec!["local".into(), "session".into()],
        agent_kind: None,
        agent_task: None,
        canonical: OnceLock::new(),
    }
}

fn manual_session_entry(config: &SessionEntryConfig) -> Entry {
    let session = config.session.as_deref().unwrap_or(&config.name);
    let path = home().join(format!(".config/herdr/sessions/{session}"));
    let mut search_terms = vec!["session".into(), session.into()];
    search_terms.extend(config.tags.iter().cloned());
    Entry {
        source: Source::Session,
        title: config.name.clone(),
        subtitle: format!("local session · {session}"),
        path,
        workspace_id: None,
        workspace_label: None,
        agent_target: None,
        project: None,
        action: EntryAction::AttachSession {
            name: session.into(),
            remote: None,
        },
        source_label: None,
        search_terms,
        agent_kind: None,
        agent_task: None,
        canonical: OnceLock::new(),
    }
}

fn remote_entry(config: &SessionEntryConfig) -> Option<Entry> {
    let target = config.remote.clone()?;
    let mut search_terms = vec!["server".into(), "remote".into(), target.clone()];
    search_terms.extend(config.tags.iter().cloned());
    Some(Entry {
        source: Source::Server,
        title: config.name.clone(),
        subtitle: format!("remote Herdr · {target}"),
        path: PathBuf::from(format!("remote:{target}")),
        workspace_id: None,
        workspace_label: None,
        agent_target: None,
        project: None,
        action: EntryAction::OpenRemote { target },
        source_label: None,
        search_terms,
        agent_kind: None,
        agent_task: None,
        canonical: OnceLock::new(),
    })
}

pub(crate) fn attach_session(name: &str) -> Result<(), String> {
    let status = herdr_attach_command()
        .args(["session", "attach", name])
        .status()
        .map_err(|err| err.to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("herdr exited with {status}"))
    }
}

pub(crate) fn open_remote(target: &str) -> Result<(), String> {
    let status = Command::new(herdr_bin())
        .args(["--remote", target, "--handoff"])
        .status()
        .map_err(|err| err.to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("herdr exited with {status}"))
    }
}

fn herdr_attach_command() -> Command {
    let mut command = Command::new(herdr_bin());
    for key in env::vars()
        .map(|(key, _)| key)
        .filter(|key| key.starts_with("HERDR_"))
    {
        command.env_remove(key);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_session_entry_attaches_by_name() {
        let entry = local_session_entry(ListedSession {
            name: "work".into(),
            running: true,
            default: false,
            session_dir: Some("/tmp/herdr-work".into()),
        });

        assert_eq!(entry.source, Source::Session);
        assert!(entry.subtitle.contains("running"));
        assert!(matches!(
            entry.action,
            EntryAction::AttachSession { ref name, remote: None } if name == "work"
        ));
    }

    #[test]
    fn manual_remote_entry_opens_remote_target() {
        let entry = remote_entry(&SessionEntryConfig {
            name: "prod".into(),
            remote: Some("prod-box".into()),
            session: Some("default".into()),
            tags: vec!["api".into()],
        })
        .unwrap();

        assert_eq!(entry.source, Source::Server);
        assert!(entry.haystack().contains("prod-box"));
        assert!(matches!(
            entry.action,
            EntryAction::OpenRemote { ref target } if target == "prod-box"
        ));
    }

    fn machine(
        label: Option<&str>,
        target: &str,
        enabled: bool,
        selected: bool,
        session: Option<&str>,
    ) -> ListedMachine {
        ListedMachine {
            label: label.map(String::from),
            target: Some(target.into()),
            session: session.map(String::from),
            enabled,
            selected,
        }
    }

    #[test]
    fn listed_machine_becomes_server_entry_with_status_flags() {
        let mut targets = HashSet::new();
        let entry = machine_entry(
            &machine(Some("Work Box"), "aliz@work", true, true, Some("main")),
            &[],
            &mut targets,
        )
        .unwrap();

        assert_eq!(entry.source, Source::Server);
        assert_eq!(entry.title, "Work Box");
        assert!(entry.subtitle.contains("connected"));
        assert!(entry.subtitle.contains("main"));
        assert!(entry.subtitle.contains("aliz@work"));
        assert!(entry.haystack().contains("work box"));
        assert!(matches!(
            entry.action,
            EntryAction::OpenRemote { ref target } if target == "aliz@work"
        ));
        assert!(targets.contains("aliz@work"));
    }

    #[test]
    fn unlabeled_machine_uses_target_as_title_and_marks_disabled() {
        let mut targets = HashSet::new();
        let entry = machine_entry(&machine(None, "build@ci", false, false, None), &[], &mut targets)
            .unwrap();

        assert_eq!(entry.title, "build@ci");
        assert!(entry.subtitle.contains("disabled"));
    }

    #[test]
    fn machine_without_target_is_skipped() {
        let mut targets = HashSet::new();
        let ghost = ListedMachine {
            label: Some("ghost".into()),
            target: None,
            session: None,
            enabled: true,
            selected: false,
        };
        assert!(machine_entry(&ghost, &[], &mut targets).is_none());
    }

    #[test]
    fn manual_entry_matching_a_listed_machine_contributes_its_tags() {
        let mut targets = HashSet::new();
        let manual = vec![SessionEntryConfig {
            name: "prod".into(),
            remote: Some("prod-box".into()),
            session: None,
            tags: vec!["api".into()],
        }];
        let entry = machine_entry(
            &machine(Some("prod"), "prod-box", true, false, None),
            &manual,
            &mut targets,
        )
        .unwrap();

        assert!(entry.search_terms.iter().any(|t| t == "api"));
    }

    #[test]
    fn listed_machines_parse_bare_array_and_machines_envelope() {
        let bare: Vec<ListedMachine> =
            serde_json::from_str(r#"[{"label":"a","target":"u@h","enabled":true}]"#).unwrap();
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].label.as_deref(), Some("a"));

        let envelope: Vec<ListedMachine> = serde_json::from_str::<serde_json::Value>(
            r#"{"machines":[{"target":"u@h2","selected":true,"enabled":true}]}"#,
        )
        .ok()
        .and_then(|json| serde_json::from_value(json.pointer("/machines").cloned().unwrap()).ok())
        .unwrap();
        assert_eq!(envelope.len(), 1);
        assert!(envelope[0].selected);
    }
}
