//! Runs `fixtures/conformance.json` through `Core::handle`, the same JSON the
//! TS runner sends to the mock. Format: CONFORMANCE.md.

mod common;

use std::collections::HashMap;

use api::Event;
use common::{Peer, World};
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("fixtures/conformance.json");

#[test]
fn conformance_fixture() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    assert_eq!(fixture["version"], 1);
    let mut ran = 0;
    for scenario in fixture["scenarios"].as_array().expect("scenarios") {
        let targets = scenario["targets"].as_array().expect("targets");
        if !targets.contains(&json!("rust")) {
            continue;
        }
        run_scenario(scenario);
        ran += 1;
    }
    assert!(ran >= 4, "ran {ran} scenarios");
}

fn run_scenario(scenario: &Value) {
    let name = scenario["name"].as_str().expect("name");
    let world = World::new();
    let peers: HashMap<String, Peer> = scenario["peers"]
        .as_array()
        .expect("peers")
        .iter()
        .map(|p| (p.as_str().expect("peer name").to_owned(), world.peer()))
        .collect();
    let mut saved: HashMap<String, Value> = HashMap::new();

    for (i, step) in scenario["steps"].as_array().expect("steps").iter().enumerate() {
        let at = format!("{name} / step {i} ({})", step["cmd"]);
        let peer = &peers[step["peer"].as_str().expect("peer")];
        peer.events.take();
        let args = substitute(step.get("args").unwrap_or(&json!({})), &saved, &at);
        let request = json!({ "cmd": step["cmd"], "args": args }).to_string();
        let response: Value =
            serde_json::from_str(&peer.core.handle(&request)).expect("JSON reply");

        let expect = substitute(&step["expect"], &saved, &at);
        if let Err(why) = matches(&expect, &response, "") {
            panic!("{at}: {why}\n  expected {expect}\n  got      {response}");
        }

        if let Some(wanted) = step.get("events") {
            let emitted: Vec<String> =
                peer.events.take().iter().map(|e: &Event| event_name(e)).collect();
            let mut rest = emitted.iter();
            for name in wanted.as_array().expect("events list") {
                let name = name.as_str().expect("event name");
                assert!(rest.any(|e| e == name), "{at}: event {name} missing from {emitted:?}");
            }
        }

        if let Some(save) = step.get("save") {
            for (var, path) in save.as_object().expect("save map") {
                let value = lookup(&response, path.as_str().expect("path"))
                    .unwrap_or_else(|| panic!("{at}: nothing at {path} in {response}"));
                saved.insert(var.clone(), value.clone());
            }
        }
    }
}

fn event_name(event: &Event) -> String {
    serde_json::to_value(event).expect("event JSON")["event"].as_str().expect("tag").to_owned()
}

/// `"${var}"` and `"${var.N}"` become the saved value.
fn substitute(value: &Value, saved: &HashMap<String, Value>, at: &str) -> Value {
    match value {
        Value::String(s) if s.starts_with("${") && s.ends_with('}') => {
            let path = &s[2..s.len() - 1];
            let (var, rest) = path.split_once('.').map_or((path, None), |(v, r)| (v, Some(r)));
            let base = saved.get(var).unwrap_or_else(|| panic!("{at}: ${{{var}}} not saved"));
            match rest {
                None => base.clone(),
                Some(rest) => {
                    lookup(base, rest).unwrap_or_else(|| panic!("{at}: nothing at {s}")).clone()
                }
            }
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| substitute(v, saved, at)).collect())
        }
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), substitute(v, saved, at))).collect())
        }
        other => other.clone(),
    }
}

/// Dotted path; numbers index arrays.
fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, key| match key.parse::<usize>() {
        Ok(i) => v.get(i),
        Err(_) => v.get(key),
    })
}

/// Objects match as subsets, arrays element by element; `"<string>"`,
/// `"<any>"` and `"<len:N>"` are wildcards.
fn matches(expected: &Value, actual: &Value, at: &str) -> Result<(), String> {
    match expected {
        Value::String(s) if s == "<any>" => Ok(()),
        Value::String(s) if s == "<string>" => {
            actual.is_string().then_some(()).ok_or(format!("{at}: not a string"))
        }
        Value::String(s) if s.starts_with("<len:") && s.ends_with('>') => {
            let n: usize = s[5..s.len() - 1].parse().map_err(|_| format!("bad wildcard {s}"))?;
            let len = actual.as_array().map(Vec::len);
            (len == Some(n)).then_some(()).ok_or(format!("{at}: length {len:?}, want {n}"))
        }
        Value::Object(map) => {
            let actual = actual.as_object().ok_or(format!("{at}: not an object"))?;
            for (key, want) in map {
                let got = actual.get(key).ok_or(format!("{at}.{key}: missing"))?;
                matches(want, got, &format!("{at}.{key}"))?;
            }
            Ok(())
        }
        Value::Array(items) => {
            let actual = actual.as_array().ok_or(format!("{at}: not an array"))?;
            if actual.len() != items.len() {
                return Err(format!("{at}: {} items, want {}", actual.len(), items.len()));
            }
            for (i, (want, got)) in items.iter().zip(actual).enumerate() {
                matches(want, got, &format!("{at}[{i}]"))?;
            }
            Ok(())
        }
        other => (other == actual).then_some(()).ok_or(format!("{at}: {actual} != {other}")),
    }
}
