#![cfg(test)]

//! Tests for the auth / pause / admin helpers in lib.rs. A minimal shim
//! contract wraps each helper so it runs inside a contract context; the
//! mock multisig answers `is_executable` with a stored flag.

use crate::{
    get_admin, is_paused, require_admin_or_multisig, require_not_paused, set_admin, set_paused,
    ActionClass, Auth,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, symbol_short,
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ShimError {
    Unauthorized = 1,
    Paused = 2,
}

const ADMIN: Symbol = symbol_short!("admin");
const PAUSED: Symbol = symbol_short!("paused");

#[contract]
pub struct MockMultisig;

#[contractimpl]
impl MockMultisig {
    pub fn set(env: Env, executable: bool) {
        env.storage()
            .instance()
            .set(&symbol_short!("ok"), &executable);
    }

    pub fn is_executable(env: Env, _action_id: u64, _class: ActionClass) -> bool {
        env.storage().instance().get(&symbol_short!("ok")).unwrap()
    }
}

#[contract]
pub struct Shim;

#[contractimpl]
impl Shim {
    pub fn init(env: Env, admin: Address) {
        env.storage().instance().set(&ADMIN, &admin);
    }

    pub fn guarded(env: Env, multisig: Option<Address>, class: u32) {
        let auth = match multisig {
            Some(m) => Auth::Multisig(m, 7),
            None => Auth::Admin,
        };
        let class = match class {
            0 => ActionClass::Emergency,
            1 => ActionClass::Standard,
            _ => ActionClass::Critical,
        };
        require_admin_or_multisig(&env, &ADMIN, &auth, class, ShimError::Unauthorized);
    }

    pub fn admin(env: Env) -> Address {
        get_admin(&env, &ADMIN)
    }

    pub fn transfer(env: Env, new_admin: Address) -> Address {
        get_admin(&env, &ADMIN).require_auth();
        set_admin(&env, &ADMIN, &new_admin)
    }

    pub fn set_paused(env: Env, paused: bool) {
        set_paused(&env, &PAUSED, paused);
    }

    pub fn paused(env: Env) -> bool {
        is_paused(&env, &PAUSED)
    }

    pub fn act(env: Env) {
        require_not_paused(&env, &PAUSED, ShimError::Paused);
    }
}

fn setup(env: &Env) -> (ShimClient<'_>, Address) {
    let id = env.register_contract(None, Shim);
    let client = ShimClient::new(env, &id);
    let admin = Address::generate(env);
    client.init(&admin);
    (client, admin)
}

fn multisig(env: &Env, executable: bool) -> Address {
    let id = env.register_contract(None, MockMultisig);
    MockMultisigClient::new(env, &id).set(&executable);
    id
}

#[test]
fn admin_arm_passes_with_admin_auth() {
    let env = Env::default();
    let (c, admin) = setup(&env);
    c.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &c.address,
            fn_name: "guarded",
            args: (None::<Address>, 1u32).into_val(&env),
            sub_invokes: &[],
        },
    }])
    .guarded(&None, &1);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn admin_arm_fails_without_admin_auth() {
    let env = Env::default();
    let (c, _) = setup(&env);
    c.guarded(&None, &1);
}

#[test]
fn multisig_arm_passes_when_executable_without_admin_auth() {
    let env = Env::default();
    let (c, _) = setup(&env);
    // No auths are mocked: success proves the admin is never consulted.
    c.guarded(&Some(multisig(&env, true)), &2);
}

#[test]
fn multisig_arm_rejects_with_exact_unauthorized_code() {
    let env = Env::default();
    let (c, admin) = setup(&env);
    env.mock_all_auths(); // admin auth available, must not be a fallback
    let _ = admin;
    for class in 0..3u32 {
        let r = c.try_guarded(&Some(multisig(&env, false)), &class);
        assert_eq!(r, Err(Ok(ShimError::Unauthorized.into())));
    }
}

#[test]
fn multisig_failure_is_not_a_generic_error() {
    let env = Env::default();
    let (c, _) = setup(&env);
    env.mock_all_auths();
    let r = c.try_guarded(&Some(multisig(&env, false)), &1);
    // A contract error (Ok(Error::Contract)) rather than a host/invoke failure.
    assert!(matches!(r, Err(Ok(e)) if e == ShimError::Unauthorized.into()));
}

#[test]
fn pause_blocks_and_unpause_allows() {
    let env = Env::default();
    let (c, _) = setup(&env);
    assert!(!c.paused());
    c.act();
    c.set_paused(&true);
    assert!(c.paused());
    assert_eq!(c.try_act(), Err(Ok(ShimError::Paused.into())));
    c.set_paused(&false);
    c.act();
}

#[test]
fn pause_flag_is_scoped_per_contract() {
    let env = Env::default();
    let (a, _) = setup(&env);
    let (b, _) = setup(&env);
    a.set_paused(&true);
    assert!(a.paused());
    assert!(!b.paused());
    b.act();
}

#[test]
fn admin_transfer_happy_path_returns_previous_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let (c, admin) = setup(&env);
    let new_admin = Address::generate(&env);
    assert_eq!(c.transfer(&new_admin), admin);
    assert_eq!(c.admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn admin_transfer_unauthorized_is_rejected() {
    let env = Env::default();
    let (c, _) = setup(&env);
    c.transfer(&Address::generate(&env));
}

#[test]
fn admin_unchanged_when_transfer_never_completed() {
    let env = Env::default();
    let (c, admin) = setup(&env);
    let new_admin = Address::generate(&env);
    // The transfer is attempted unauthorized and fails, so nothing moves.
    assert!(c.try_transfer(&new_admin).is_err());
    assert_eq!(c.admin(), admin);
}
