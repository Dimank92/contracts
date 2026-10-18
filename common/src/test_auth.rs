#![cfg(test)]

//! `require_admin_or_multisig`, one-shot approvals and the two-step admin
//! transfer, against a throwaway consumer contract and a stub multisig.

extern crate std;

use crate::{
    accept_admin, is_approval_used, pending_admin, propose_admin, require_admin_or_multisig,
    ActionClass, Auth,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, symbol_short,
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, BytesN, Env, IntoVal,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ConsumerError {
    Unauthorized = 1,
    NoPendingAdmin = 2,
}

#[contract]
pub struct StubMultisig;

#[contractimpl]
impl StubMultisig {
    pub fn set(env: Env, action_id: BytesN<32>, approved: bool) {
        env.storage().instance().set(&action_id, &approved);
    }

    pub fn is_approved(env: Env, action_id: BytesN<32>) -> bool {
        env.storage().instance().get(&action_id).unwrap_or(false)
    }
}

#[contract]
pub struct Consumer;

fn bump_inner(env: &Env, auth: Auth) {
    require_admin_or_multisig(
        env,
        &symbol_short!("admin"),
        &auth,
        ActionClass::Standard,
        ConsumerError::Unauthorized,
    );
    let n: u32 = env
        .storage()
        .instance()
        .get(&symbol_short!("n"))
        .unwrap_or(0);
    env.storage().instance().set(&symbol_short!("n"), &(n + 1));
}

#[contractimpl]
impl Consumer {
    pub fn init(env: Env, admin: Address, multisig: Address) {
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
        crate::set_trusted_multisig(&env, &multisig);
    }

    pub fn bump(env: Env) {
        bump_inner(&env, Auth::Admin);
    }

    pub fn bump_via_multisig(env: Env, multisig: Address, action_id: BytesN<32>) {
        bump_inner(&env, Auth::Multisig(multisig, action_id));
    }

    pub fn count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("n"))
            .unwrap_or(0)
    }

    pub fn used(env: Env, multisig: Address, action_id: BytesN<32>) -> bool {
        is_approval_used(&env, &multisig, &action_id)
    }

    pub fn transfer_admin(env: Env, new_admin: Address) {
        require_admin_or_multisig(
            &env,
            &symbol_short!("admin"),
            &Auth::Admin,
            ActionClass::Critical,
            ConsumerError::Unauthorized,
        );
        propose_admin(&env, &symbol_short!("admin"), &new_admin);
    }

    pub fn accept_admin(env: Env) {
        accept_admin(&env, &symbol_short!("admin"), ConsumerError::NoPendingAdmin);
    }

    pub fn admin(env: Env) -> Address {
        crate::get_admin(&env, &symbol_short!("admin"))
    }

    pub fn pending(env: Env) -> Option<Address> {
        pending_admin(&env)
    }
}

struct World<'a> {
    env: Env,
    admin: Address,
    consumer: ConsumerClient<'a>,
    multisig: StubMultisigClient<'a>,
}

fn setup<'a>() -> World<'a> {
    let env = Env::default();
    let admin = Address::generate(&env);
    let consumer = ConsumerClient::new(&env, &env.register_contract(None, Consumer));
    let multisig = StubMultisigClient::new(&env, &env.register_contract(None, StubMultisig));
    consumer.init(&admin, &multisig.address);
    World {
        env,
        admin,
        consumer,
        multisig,
    }
}

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

fn sign(w: &World, who: &Address, fn_name: &str, args: soroban_sdk::Vec<soroban_sdk::Val>) {
    w.env.mock_auths(&[MockAuth {
        address: who,
        invoke: &MockAuthInvoke {
            contract: &w.consumer.address,
            fn_name,
            args,
            sub_invokes: &[],
        },
    }]);
}

#[test]
fn admin_path_requires_the_stored_admin() {
    let w = setup();
    sign(&w, &w.admin, "bump", ().into_val(&w.env));
    w.consumer.bump();
    assert_eq!(w.consumer.count(), 1);

    let stranger = Address::generate(&w.env);
    sign(&w, &stranger, "bump", ().into_val(&w.env));
    assert!(w.consumer.try_bump().is_err());
    assert_eq!(w.consumer.count(), 1);
}

#[test]
fn multisig_path_accepts_an_approved_action_once() {
    let w = setup();
    let id = aid(&w.env, 1);
    w.multisig.set(&id, &true);

    w.consumer.bump_via_multisig(&w.multisig.address, &id);
    assert_eq!(w.consumer.count(), 1);
    assert!(w.consumer.used(&w.multisig.address, &id));

    // Replaying the same approval is rejected.
    assert_eq!(
        w.consumer.try_bump_via_multisig(&w.multisig.address, &id),
        Err(Ok(ConsumerError::Unauthorized.into()))
    );
    assert_eq!(w.consumer.count(), 1);
}

#[test]
fn multisig_path_rejects_an_unapproved_action() {
    let w = setup();
    let id = aid(&w.env, 2);
    assert_eq!(
        w.consumer.try_bump_via_multisig(&w.multisig.address, &id),
        Err(Ok(ConsumerError::Unauthorized.into()))
    );
    assert!(!w.consumer.used(&w.multisig.address, &id));
}

#[test]
fn only_the_trusted_multisig_is_accepted() {
    let w = setup();
    let id = aid(&w.env, 3);
    // A multisig the caller deployed themselves, approving everything.
    let rogue = StubMultisigClient::new(&w.env, &w.env.register_contract(None, StubMultisig));
    rogue.set(&id, &true);
    assert_eq!(
        w.consumer.try_bump_via_multisig(&rogue.address, &id),
        Err(Ok(ConsumerError::Unauthorized.into()))
    );
    assert_eq!(w.consumer.count(), 0);
}

#[test]
fn two_step_transfer_only_completes_when_the_new_admin_accepts() {
    let w = setup();
    let next = Address::generate(&w.env);
    sign(
        &w,
        &w.admin,
        "transfer_admin",
        (next.clone(),).into_val(&w.env),
    );
    w.consumer.transfer_admin(&next);
    assert_eq!(w.consumer.admin(), w.admin);
    assert_eq!(w.consumer.pending(), Some(next.clone()));

    // Nobody but the pending admin can accept.
    sign(&w, &w.admin, "accept_admin", ().into_val(&w.env));
    assert!(w.consumer.try_accept_admin().is_err());

    sign(&w, &next, "accept_admin", ().into_val(&w.env));
    w.consumer.accept_admin();
    assert_eq!(w.consumer.admin(), next);
    assert_eq!(w.consumer.pending(), None);
}

#[test]
fn proposing_the_current_admin_cancels_a_pending_transfer() {
    let w = setup();
    w.env.mock_all_auths();
    let wrong = Address::generate(&w.env);
    w.consumer.transfer_admin(&wrong);
    w.consumer.transfer_admin(&w.admin);
    assert_eq!(w.consumer.pending(), None);
    assert_eq!(
        w.consumer.try_accept_admin(),
        Err(Ok(ConsumerError::NoPendingAdmin.into()))
    );
}
