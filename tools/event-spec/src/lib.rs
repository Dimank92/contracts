//! Spec conformance for contract events (#126).
//!
//! `events.json` at the repo root is the machine-readable event spec.
//! A contract's event tests call [`Recorder::record`] after each
//! invocation; it checks every event that contract emitted against the
//! spec (topic layout and types, schema version, data struct fields and
//! types) and panics with a precise message on any mismatch. At the end,
//! [`Recorder::assert_complete`] fails unless every event the spec lists
//! for that contract was emitted at least once, so the spec can't
//! describe an event no test exercises.
//!
//! Type strings in events.json: `address`, `symbol`, `bool`, `u32`,
//! `u64`, `i128`, `bytes32`, `bytes`, `string`, `val` (anything),
//! `vec<T>`, `map<K,V>`, `option<T>`, `tuple<A,B,..>`, or the name of a
//! struct or enum in the contract's (or the shared) `types`.
//!
//! An event with `"versioned": false` (lp_token's standard SEP-41 events)
//! has topics `(name, keys...)` with no schema-version topic.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use soroban_sdk::{
    testutils::Events as _,
    xdr::{ScMapEntry, ScVal},
    Address, Env, TryFromVal, Val,
};

/// events.json, embedded at compile time.
pub const EVENTS_JSON: &str = include_str!("../../../events.json");

#[derive(Debug, Deserialize)]
pub struct Spec {
    pub schema_version: u32,
    pub shared: Contract,
    pub contracts: BTreeMap<String, Contract>,
}

#[derive(Debug, Deserialize)]
pub struct Contract {
    #[serde(default)]
    pub types: BTreeMap<String, TypeDef>,
    pub events: Vec<Event>,
    /// Names of shared (framework) events this contract can emit.
    #[serde(default)]
    pub shared_events: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Event {
    pub name: String,
    /// Topics after `(name, version)`.
    pub topics: Vec<Field>,
    /// Type of the data: a struct in `types`.
    pub data: String,
    /// False for standard events (SEP-41) that keep their own layout.
    #[serde(default = "yes")]
    pub versioned: bool,
    /// The state change this event represents.
    pub state: String,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TypeDef {
    Struct {
        fields: Vec<Field>,
    },
    /// A `#[repr(u32)]` enum, encoded as its `u32` discriminant.
    Enum {
        variants: BTreeMap<String, u32>,
    },
}

#[derive(Debug, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

pub fn spec() -> Spec {
    serde_json::from_str(EVENTS_JSON).unwrap_or_else(|e| panic!("events.json: {e}"))
}

/// Checks and tallies one contract's events.
pub struct Recorder {
    spec: Spec,
    contract: String,
    address: Address,
    seen: BTreeSet<String>,
}

impl Recorder {
    /// `contract` is its key in events.json (`params`, `multisig`, ...).
    pub fn new(contract: &str, address: &Address) -> Recorder {
        let spec = spec();
        assert!(
            spec.contracts.contains_key(contract),
            "events.json has no contract `{contract}`"
        );
        Recorder {
            spec,
            contract: contract.to_string(),
            address: address.clone(),
            seen: BTreeSet::new(),
        }
    }

    /// For zenith-common's framework events, checked against `shared`.
    pub fn shared(address: &Address) -> Recorder {
        Recorder {
            spec: spec(),
            contract: String::new(),
            address: address.clone(),
            seen: BTreeSet::new(),
        }
    }

    fn contract(&self) -> &Contract {
        if self.contract.is_empty() {
            &self.spec.shared
        } else {
            &self.spec.contracts[&self.contract]
        }
    }

    /// Checks every event this contract has emitted so far in `env`.
    pub fn record(&mut self, env: &Env) {
        let mut seen = Vec::new();
        for (address, topics, data) in env.events().all().iter() {
            if address != self.address {
                continue;
            }
            let topics: Vec<ScVal> = topics.iter().map(|t| to_scval(env, &t)).collect();
            let data = to_scval(env, &data);
            seen.push(self.check(&topics, &data));
        }
        self.seen.extend(seen);
    }

    /// Validates one event and returns its name.
    fn check(&self, topics: &[ScVal], data: &ScVal) -> String {
        let name = match topics.first() {
            Some(ScVal::Symbol(s)) => s.to_utf8_string_lossy(),
            other => panic!("event's first topic must be its name symbol, got {other:?}"),
        };
        let ctx = format!("{}: event `{name}`", self.label());
        let contract = self.contract();
        let (event, types) = if let Some(e) = contract.events.iter().find(|e| e.name == name) {
            (e, &contract.types)
        } else if contract.shared_events.contains(&name) || self.contract.is_empty() {
            let e = self
                .spec
                .shared
                .events
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{ctx}: not in events.json"));
            (e, &self.spec.shared.types)
        } else {
            panic!("{ctx}: not in events.json for this contract")
        };
        let keys_from = if event.versioned {
            match topics.get(1) {
                Some(ScVal::U32(v)) if *v == self.spec.schema_version => {}
                other => panic!(
                    "{ctx}: second topic must be u32 {}, got {other:?}",
                    self.spec.schema_version
                ),
            }
            2
        } else {
            1
        };
        assert_eq!(
            topics.len(),
            keys_from + event.topics.len(),
            "{ctx}: expected topics (name, {}{})",
            if event.versioned { "version, " } else { "" },
            event
                .topics
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (field, value) in event.topics.iter().zip(&topics[keys_from..]) {
            self.check_type(
                types,
                &field.ty,
                value,
                &format!("{ctx} topic `{}`", field.name),
            );
        }
        self.check_type(types, &event.data, data, &format!("{ctx} data"));
        name
    }

    fn label(&self) -> &str {
        if self.contract.is_empty() {
            "shared"
        } else {
            &self.contract
        }
    }

    fn check_type(&self, types: &BTreeMap<String, TypeDef>, ty: &str, v: &ScVal, ctx: &str) {
        let ty = ty.trim();
        let bad = || panic!("{ctx}: expected `{ty}`, got {v:?}");
        let inner = |prefix: &str| -> Option<&str> {
            ty.strip_prefix(prefix).and_then(|r| r.strip_suffix('>'))
        };
        match ty {
            "val" => {}
            "address" => matches!(v, ScVal::Address(_))
                .then_some(())
                .unwrap_or_else(bad),
            "symbol" => matches!(v, ScVal::Symbol(_))
                .then_some(())
                .unwrap_or_else(bad),
            "bool" => matches!(v, ScVal::Bool(_))
                .then_some(())
                .unwrap_or_else(bad),
            "u32" => matches!(v, ScVal::U32(_)).then_some(()).unwrap_or_else(bad),
            "u64" => matches!(v, ScVal::U64(_)).then_some(()).unwrap_or_else(bad),
            "i128" => matches!(v, ScVal::I128(_))
                .then_some(())
                .unwrap_or_else(bad),
            "string" => matches!(v, ScVal::String(_))
                .then_some(())
                .unwrap_or_else(bad),
            "bytes" => matches!(v, ScVal::Bytes(_))
                .then_some(())
                .unwrap_or_else(bad),
            "bytes32" => match v {
                ScVal::Bytes(b) if b.len() == 32 => {}
                _ => bad(),
            },
            _ if inner("option<").is_some() => {
                if !matches!(v, ScVal::Void) {
                    self.check_type(types, inner("option<").unwrap_or_default(), v, ctx);
                }
            }
            _ if inner("tuple<").is_some() => match v {
                ScVal::Vec(Some(items)) => {
                    let parts = split_list(inner("tuple<").unwrap_or_default());
                    assert_eq!(
                        items.len(),
                        parts.len(),
                        "{ctx}: expected `{ty}`, got {v:?}"
                    );
                    for (i, (t, item)) in parts.iter().zip(items.iter()).enumerate() {
                        self.check_type(types, t, item, &format!("{ctx}.{i}"));
                    }
                }
                _ => bad(),
            },
            _ if inner("vec<").is_some() => match v {
                ScVal::Vec(Some(items)) => {
                    let t = inner("vec<").unwrap_or_default();
                    for (i, item) in items.iter().enumerate() {
                        self.check_type(types, t, item, &format!("{ctx}[{i}]"));
                    }
                }
                _ => bad(),
            },
            _ if inner("map<").is_some() => match v {
                ScVal::Map(Some(entries)) => {
                    let kv = inner("map<").unwrap_or_default();
                    let (k, val) = split_pair(kv).unwrap_or_else(|| panic!("bad type `{ty}`"));
                    for ScMapEntry { key, val: value } in entries.iter() {
                        self.check_type(types, k, key, &format!("{ctx} key"));
                        self.check_type(types, val, value, &format!("{ctx} value"));
                    }
                }
                _ => bad(),
            },
            name => {
                let def = types
                    .get(name)
                    .or_else(|| self.spec.shared.types.get(name))
                    .unwrap_or_else(|| panic!("{ctx}: unknown type `{name}` in events.json"));
                match def {
                    TypeDef::Enum { variants } => match v {
                        ScVal::U32(d) if variants.values().any(|x| x == d) => {}
                        _ => bad(),
                    },
                    TypeDef::Struct { fields } => {
                        let ScVal::Map(Some(entries)) = v else {
                            return bad();
                        };
                        let mut got: BTreeMap<String, &ScVal> = BTreeMap::new();
                        for e in entries.iter() {
                            let ScVal::Symbol(k) = &e.key else {
                                panic!("{ctx}: struct `{name}` has a non-symbol key")
                            };
                            got.insert(k.to_utf8_string_lossy(), &e.val);
                        }
                        let want: BTreeSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                        let have: BTreeSet<&str> = got.keys().map(String::as_str).collect();
                        assert_eq!(have, want, "{ctx}: `{name}` fields differ from events.json");
                        for f in fields {
                            self.check_type(
                                types,
                                &f.ty,
                                got[&f.name],
                                &format!("{ctx}.{}", f.name),
                            );
                        }
                    }
                }
            }
        }
    }

    /// Fails unless every event events.json lists for this contract was
    /// recorded at least once.
    pub fn assert_complete(&self) {
        let missing: Vec<&str> = self
            .contract()
            .events
            .iter()
            .map(|e| e.name.as_str())
            .filter(|n| !self.seen.contains(*n))
            .collect();
        assert!(
            missing.is_empty(),
            "{}: events in events.json never emitted by this test: {}",
            self.label(),
            missing.join(", ")
        );
    }

    /// `assert_complete`, except for `elsewhere`: events another test
    /// covers (say why at the call site).
    pub fn assert_complete_except(&self, elsewhere: &[&str]) {
        let missing: Vec<&str> = self
            .contract()
            .events
            .iter()
            .map(|e| e.name.as_str())
            .filter(|n| !self.seen.contains(*n) && !elsewhere.contains(n))
            .collect();
        assert!(
            missing.is_empty(),
            "{}: events in events.json never emitted by this test: {}",
            self.label(),
            missing.join(", ")
        );
    }

    pub fn seen(&self) -> &BTreeSet<String> {
        &self.seen
    }
}

fn to_scval(env: &Env, v: &Val) -> ScVal {
    ScVal::try_from_val(env, v).unwrap_or_else(|e| panic!("event value isn't valid XDR: {e:?}"))
}

/// Splits a type list at its top-level commas.
fn split_list(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some((head, tail)) = split_pair(rest) {
        out.push(head.trim());
        rest = tail;
    }
    out.push(rest.trim());
    out
}

/// Splits `K,V` at the top-level comma.
fn split_pair(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

/// The most recent event `address` emitted, as `(topics, data)`.
pub fn last_event(env: &Env, address: &Address) -> (soroban_sdk::Vec<Val>, Val) {
    let all = env.events().all();
    let mut found = None;
    for (a, topics, data) in all.iter() {
        if a == *address {
            found = Some((topics, data));
        }
    }
    found.unwrap_or_else(|| panic!("{address:?} emitted no events"))
}

/// Every event `address` emitted named `name`, decoded as `T`.
pub fn events_named<T: TryFromVal<Env, Val>>(env: &Env, address: &Address, name: &str) -> Vec<T> {
    let mut out = Vec::new();
    for (a, topics, data) in env.events().all().iter() {
        if a != *address {
            continue;
        }
        let Some(first) = topics.first() else {
            continue;
        };
        if let ScVal::Symbol(s) = to_scval(env, &first) {
            if s.to_utf8_string_lossy() == name {
                out.push(
                    T::try_from_val(env, &data)
                        .unwrap_or_else(|_| panic!("event `{name}` data doesn't decode")),
                );
            }
        }
    }
    out
}
