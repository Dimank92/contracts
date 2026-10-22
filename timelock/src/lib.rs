//! Timelock contract: delayed-execution admin holding the admin role on other
//! contracts so users always get an exit window.
//!
//! Roles: proposer, executor, canceller, and guardian. The guardian can cancel
//! a queued proposal at any time before execution, independent of the canceller.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

mod common {
    use soroban_sdk::{Address, Env};

    /// Gate config entrypoints through the admin/multisig check.
    pub fn require_admin_or_multisig(env: &Env, auth: &Address) {
        auth.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(env, "admin"))
            .expect("admin not set");
        if &admin != auth {
            panic!("unauthorized: admin only");
        }
    }
}

/// Minimum delay floor: 1 hour. Delays may be lengthened but never shortened
/// below this safe exit window.
const MIN_DELAY_FLOOR: u64 = 3_600;

#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Proposer = 0,
    Executor = 1,
    Canceller = 2,
    Guardian = 3,
}

#[contracttype]
#[derive(Clone)]
pub struct Proposal {
    pub proposer: Address,
    pub eta: u64,
    pub executed: bool,
    pub cancelled: bool,
}

fn role_key(env: &Env, role: Role) -> Symbol {
    match role {
        Role::Proposer => Symbol::new(env, "role_proposer"),
        Role::Executor => Symbol::new(env, "role_executor"),
        Role::Canceller => Symbol::new(env, "role_canceller"),
        Role::Guardian => Symbol::new(env, "role_guardian"),
    }
}

fn get_role(env: &Env, role: Role) -> Address {
    env.storage()
        .instance()
        .get(&role_key(env, role))
        .expect("role not set")
}

fn set_role_internal(env: &Env, role: Role, new_holder: &Address) {
    let old = get_role(env, role);
    env.storage().instance().set(&role_key(env, role), new_holder);
    env.events().publish(
        (Symbol::new(env, "role_changed"), role as u32),
        (old, new_holder.clone()),
    );
}

fn get_min_delay(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "min_delay"))
        .unwrap_or(MIN_DELAY_FLOOR)
}

fn proposal_key(env: &Env, id: u64) -> (Symbol, u64) {
    (Symbol::new(env, "proposal"), id)
}

#[contract]
pub struct Timelock;

#[contractimpl]
impl Timelock {
    pub fn initialize(
        env: Env,
        admin: Address,
        proposer: Address,
        executor: Address,
        canceller: Address,
        guardian: Address,
        min_delay: u64,
    ) {
        if env.storage().instance().has(&Symbol::new(&env, "admin")) {
            panic!("already initialized");
        }
        if min_delay < MIN_DELAY_FLOOR {
            panic!("min_delay below floor");
        }
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "min_delay"), &min_delay);
        env.storage().instance().set(&role_key(&env, Role::Proposer), &proposer);
        env.storage().instance().set(&role_key(&env, Role::Executor), &executor);
        env.storage().instance().set(&role_key(&env, Role::Canceller), &canceller);
        env.storage().instance().set(&role_key(&env, Role::Guardian), &guardian);
    }

    /// Admin-settable minimum delay. Enforces the floor so the exit window can
    /// be lengthened but never shortened below a safe minimum. Does not affect
    /// already-queued proposals (their `eta` is captured at queue time).
    pub fn set_min_delay(env: Env, auth: Address, delay: u64) {
        common::require_admin_or_multisig(&env, &auth);
        if delay < MIN_DELAY_FLOOR {
            panic!("min_delay below floor");
        }
        let old = get_min_delay(&env);
        env.storage().instance().set(&Symbol::new(&env, "min_delay"), &delay);
        env.events().publish(
            (Symbol::new(&env, "min_delay_changed"),),
            (old, delay),
        );
    }

    /// Rotate a single role holder. Rotating to the current holder is a no-op
    /// (still emits an event); rotating to the zero address is rejected.
    pub fn set_role(env: Env, auth: Address, role: Role, address: Address) {
        common::require_admin_or_multisig(&env, &auth);
        if address == Address::from_string(&soroban_sdk::String::from_str(&env, "")) {
            panic!("cannot rotate role to zero address");
        }
        set_role_internal(&env, role, &address);
    }

    /// Rotate all four role holders in one call.
    pub fn rotate_roles(
        env: Env,
        auth: Address,
        proposer: Address,
        executor: Address,
        canceller: Address,
        guardian: Address,
    ) {
        common::require_admin_or_multisig(&env, &auth);
        set_role_internal(&env, Role::Proposer, &proposer);
        set_role_internal(&env, Role::Executor, &executor);
        set_role_internal(&env, Role::Canceller, &canceller);
        set_role_internal(&env, Role::Guardian, &guardian);
    }

    /// Queue a proposal, capturing `eta = now + min_delay` at queue time so
    /// later delay changes are non-retroactive by construction.
    pub fn queue(env: Env, auth: Address, id: u64) -> u64 {
        auth.require_auth();
        let proposer = get_role(&env, Role::Proposer);
        if auth != proposer {
            panic!("unauthorized: proposer only");
        }
        let eta = env.ledger().timestamp() + get_min_delay(&env);
        let proposal = Proposal {
            proposer: auth,
            eta,
            executed: false,
            cancelled: false,
        };
        env.storage().persistent().set(&proposal_key(&env, id), &proposal);
        env.events().publish((Symbol::new(&env, "queued"), id), eta);
        eta
    }

    /// Guardian veto: cancel a queued proposal any time before execution,
    /// independent of the canceller role.
    pub fn guardian_cancel(env: Env, auth: Address, proposal_id: u64) {
        auth.require_auth();
        let guardian = get_role(&env, Role::Guardian);
        if auth != guardian {
            panic!("unauthorized: guardian only");
        }
        let key = proposal_key(&env, proposal_id);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&key)
            .expect("proposal not found");
        if proposal.executed {
            panic!("proposal already executed");
        }
        if proposal.cancelled {
            panic!("proposal already cancelled");
        }
        proposal.cancelled = true;
        env.storage().persistent().set(&key, &proposal);
        env.events()
            .publish((Symbol::new(&env, "guardian_cancelled"), proposal_id), auth);
    }

    pub fn get_min_delay(env: Env) -> u64 {
        get_min_delay(&env)
    }

    pub fn get_role(env: Env, role: Role) -> Address {
        get_role(&env, role)
    }

    pub fn get_proposal(env: Env, id: u64) -> Proposal {
        env.storage()
            .persistent()
            .get(&proposal_key(&env, id))
            .expect("proposal not found")
    }
}
