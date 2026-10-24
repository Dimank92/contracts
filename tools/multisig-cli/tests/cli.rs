//! Drives the CLI logic against the real `multisig` contract running in the
//! Soroban test environment (no network).

use multisig::{Multisig, MultisigClient, DEFAULT_ROTATION_DELAY};
use multisig_cli::{
    action_id, collect, payload, status, submit, Action, Approver, Backend, Result,
};
use serde_json::{json, Map, Value};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Map as SMap, String as SString, Symbol, Val, Vec as SVec,
};
use std::cell::RefCell;

struct TestBackend<'a> {
    env: &'a Env,
    client: MultisigClient<'a>,
    contract: String,
    identities: Vec<(String, Address)>,
    sent: RefCell<Vec<String>>,
}

impl TestBackend<'_> {
    fn addr(&self, s: &str) -> Address {
        Address::from_string(&SString::from_str(self.env, s))
    }

    fn id(&self, hex: &str) -> BytesN<32> {
        let bytes: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        BytesN::from_array(self.env, &bytes.try_into().unwrap())
    }
}

fn arg<'a>(args: &'a [(&str, String)], name: &str) -> &'a str {
    &args.iter().find(|(n, _)| *n == name).unwrap().1
}

impl Backend for TestBackend<'_> {
    fn read(&self, _c: &str, func: &str, args: &[(&str, String)]) -> Result<Value> {
        let id = || self.id(arg(args, "action_id"));
        Ok(match func {
            "get_signers" => Value::Array(
                self.client
                    .get_signers()
                    .iter()
                    .map(|(a, w)| json!({ "key": a.to_string().to_string(), "val": w }))
                    .collect(),
            ),
            "has_approved" => json!(self
                .client
                .has_approved(&id(), &self.addr(arg(args, "signer")))),
            "is_approved" => json!(self.client.is_approved(&id())),
            "get_approval_count" => json!(self.client.get_approval_count(&id())),
            "get_approval_weight" => json!(self.client.get_approval_weight(&id())),
            "get_threshold" => json!(self.client.get_threshold()),
            "get_approval_ttl" => json!(self.client.get_approval_ttl()),
            "get_execute_by" => json!(self.client.get_execute_by(&id())),
            other => return Err(format!("unsupported read {other}")),
        })
    }

    fn send(
        &self,
        contract: &str,
        func: &str,
        args: &[(&str, String)],
        _source: &str,
    ) -> Result<String> {
        if contract == self.contract && func == "approve" {
            self.client.approve(
                &self.addr(arg(args, "signer")),
                &self.id(arg(args, "action_id")),
            );
        }
        self.sent.borrow_mut().push(format!("{contract}.{func}"));
        Ok("tx-ok".into())
    }

    fn send_signed(&self, xdr: &str) -> Result<String> {
        self.sent.borrow_mut().push(format!("signed:{xdr}"));
        Ok("tx-signed".into())
    }

    fn address_of(&self, identity: &str) -> Result<String> {
        let (_, a) = self
            .identities
            .iter()
            .find(|(n, _)| n == identity)
            .ok_or("unknown identity")?;
        Ok(a.to_string().to_string())
    }
}

fn setup(env: &Env, ttl: u64) -> TestBackend<'_> {
    env.mock_all_auths();
    let signers: Vec<Address> = (0..3).map(|_| Address::generate(env)).collect();
    let id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(env, &id);
    client.initialize(
        &vec![
            env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone(),
        ],
        &2,
        &ttl,
    );
    TestBackend {
        env,
        client,
        contract: id.to_string().to_string(),
        identities: ["alice", "bob", "carol"]
            .iter()
            .map(|s| s.to_string())
            .zip(signers)
            .collect(),
        sent: RefCell::default(),
    }
}

fn who(names: &[&str]) -> Vec<Approver> {
    names
        .iter()
        .map(|n| Approver::Identity(n.to_string()))
        .collect()
}

fn pause_action(args: &Map<String, Value>) -> Action<'_> {
    Action {
        target: "CTARGET",
        function: "pause_via_multisig",
        args,
    }
}

#[test]
fn payload_is_deterministic_and_matches_golden() {
    let args = json!({ "b": "2", "a": "1" }).as_object().unwrap().clone();
    let a = pause_action(&args);
    let id = action_id(&a);
    assert_eq!(
        id,
        action_id(&pause_action(
            &json!({ "a": "1", "b": "2" }).as_object().unwrap().clone()
        ))
    );
    let p = payload("--rpc-url R", "CMULTI", &a, &["alice".into()]);
    assert_eq!(
        p,
        json!({
            "multisig": "CMULTI", "action_id": id, "target": "CTARGET",
            "function": "pause_via_multisig", "args": { "a": "1", "b": "2" },
            "approve": [{ "signer": "alice", "command": format!("stellar contract invoke --id CMULTI --rpc-url R --source alice --build-only -- approve --signer alice --action_id {id}") }],
        })
    );
}

#[test]
fn happy_path_collects_threshold_then_submits() {
    let env = Env::default();
    let b = setup(&env, 0);
    let args = Map::new();
    let a = pause_action(&args);
    let id = action_id(&a);

    let r = collect(&b, &b.contract, &id, &who(&["alice", "bob"]), false).unwrap();
    assert_eq!(r["approved"], true);
    let st = status(&b, &b.contract, &id, None).unwrap();
    assert_eq!(
        (
            st["approval_count"].clone(),
            st["approval_weight"].clone(),
            st["threshold"].clone(),
            st["approved"].clone()
        ),
        (json!(2), json!(2), json!(2), json!(true))
    );
    assert_eq!(
        st["signers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["approved"] == true)
            .count(),
        2
    );

    let out = submit(&b, &b.contract, &id, &a, None, "alice", false).unwrap();
    assert_eq!(
        out,
        json!({ "action_id": id, "dry_run": false, "tx": "tx-ok" })
    );
    assert_eq!(
        b.sent.borrow().last().unwrap(),
        "CTARGET.pause_via_multisig"
    );
}

#[test]
fn one_signature_short_is_refused() {
    let env = Env::default();
    let b = setup(&env, 0);
    let args = Map::new();
    let a = pause_action(&args);
    let id = action_id(&a);
    let r = collect(&b, &b.contract, &id, &who(&["alice"]), false).unwrap();
    assert_eq!(r["approved"], false);
    let err = submit(&b, &b.contract, &id, &a, None, "alice", false).unwrap_err();
    assert!(
        err.starts_with("not approved: weight 1 of threshold 2"),
        "{err}"
    );
    assert_eq!(b.sent.borrow().len(), 1, "only the approval was sent");
}

#[test]
fn expired_approval_is_refused() {
    let env = Env::default();
    let b = setup(&env, 100);
    let args = Map::new();
    let a = pause_action(&args);
    let id = action_id(&a);
    collect(&b, &b.contract, &id, &who(&["alice", "bob"]), false).unwrap();
    assert_eq!(
        status(&b, &b.contract, &id, None).unwrap()["approved"],
        true
    );
    env.ledger().with_mut(|l| l.timestamp += 101);
    let st = status(&b, &b.contract, &id, None).unwrap();
    assert_eq!(
        (st["approved"].clone(), st["approval_ttl"].clone()),
        (json!(false), json!(100))
    );
    assert!(submit(&b, &b.contract, &id, &a, None, "alice", false).is_err());
}

#[test]
fn signer_set_change_mid_flight_invalidates_collection() {
    let env = Env::default();
    let b = setup(&env, 0);
    let args = Map::new();
    let a = pause_action(&args);
    let id = action_id(&a);
    collect(&b, &b.contract, &id, &who(&["alice", "bob"]), false).unwrap();

    // Rotate carol out for a newcomer (needs all three signers, then the delay).
    let newcomer = Address::generate(&env);
    let carol = b.identities[2].1.clone();
    // `SignerWeight` is private to the contract crate, so build its map encoding.
    let mut weight = SMap::<Symbol, Val>::new(&env);
    weight.set(Symbol::new(&env, "address"), newcomer.into_val(&env));
    weight.set(Symbol::new(&env, "weight"), 1u32.into_val(&env));
    let add: SVec<Val> = vec![&env, weight.into_val(&env)];
    let call: SVec<Val> = vec![
        &env,
        b.identities[0].1.into_val(&env),
        add.into_val(&env),
        vec![&env, carol].into_val(&env),
        2u32.into_val(&env),
        3u32.into_val(&env),
    ];
    let change: BytesN<32> = env.invoke_contract(
        &b.client.address,
        &Symbol::new(&env, "propose_signer_change"),
        call,
    );
    for (_, s) in &b.identities {
        b.client.approve(s, &change);
    }
    b.client.queue_signer_change(&change);
    env.ledger()
        .with_mut(|l| l.timestamp += DEFAULT_ROTATION_DELAY + 1);
    b.client.execute_signer_change(&change);

    assert!(submit(&b, &b.contract, &id, &a, None, "alice", false)
        .unwrap_err()
        .starts_with("not approved"));
    let err = collect(&b, &b.contract, &id, &who(&["carol"]), false).unwrap_err();
    assert!(err.contains("not a signer"), "{err}");
}

#[test]
fn dry_run_and_presigned_paths_and_payload_mismatch() {
    let env = Env::default();
    let b = setup(&env, 0);
    let args = Map::new();
    let a = pause_action(&args);
    let id = action_id(&a);

    let r = collect(&b, &b.contract, &id, &who(&["alice", "bob"]), true).unwrap();
    assert_eq!(
        (r["approved"].clone(), b.sent.borrow().len()),
        (json!(false), 0)
    );

    let r = collect(
        &b,
        &b.contract,
        &id,
        &[Approver::Signed("AAAA".into())],
        false,
    )
    .unwrap();
    assert_eq!(
        r["submitted"],
        json!([{ "signer": null, "tx": "tx-signed" }])
    );

    let other = Action {
        target: "COTHER",
        ..pause_action(&args)
    };
    assert!(submit(&b, &b.contract, &id, &other, None, "alice", true)
        .unwrap_err()
        .contains("does not match"));
}
