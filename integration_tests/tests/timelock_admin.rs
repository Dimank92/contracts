//! The timelock as the live admin of options_market, price_oracle and a
//! vault (issue #177): the two-step hand-off, propose → rejected before
//! the delay → executed after, cancellation, predecessor ordering, the
//! failure mode of handing off to a wrong address, and the emergency
//! pause path that stays short while the admin is delayed.
//!
//! The consumer contracts know nothing about timelocks: their admin is
//! just an address, and the timelock satisfies `admin.require_auth()` by
//! being the direct invoker of the calls it executes.

mod fixtures;

use fixtures::{deploy, Protocol, DAY, UNIT};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, IntoVal, Vec};
use timelock::TimelockClient;
use vault::{Vault, VaultClient};

const DELAY: u64 = 2 * DAY;
const GRACE: u64 = 7 * DAY;

struct World<'a> {
    p: Protocol<'a>,
    /// Timelock proposer and guardian (in production: the multisig account).
    proposer: Address,
    tl: TimelockClient<'a>,
    /// A vault administered directly (not through the market), e.g. a
    /// treasury vault, to show the vault side of the hand-off.
    treasury: VaultClient<'a>,
}

fn world<'a>() -> World<'a> {
    let p = deploy();
    let proposer = Address::generate(&p.env);
    let tl = p.timelock(&proposer, DELAY, GRACE);
    let treasury = VaultClient::new(&p.env, &p.env.register_contract(None, Vault));
    treasury.initialize(&p.admin, &p.usdc.address);
    treasury.set_multisig(&p.multisig.address);
    World {
        p,
        proposer,
        tl,
        treasury,
    }
}

impl World<'_> {
    fn admin_contracts(&self) -> [Address; 3] {
        [
            self.p.market.address.clone(),
            self.p.oracle.address.clone(),
            self.treasury.address.clone(),
        ]
    }

    /// Step one, by the current admin key: propose the timelock.
    fn propose_timelock(&self, new_admin: &Address) {
        self.p.market.transfer_admin(new_admin);
        self.p.oracle.transfer_admin(new_admin);
        self.treasury.transfer_admin(new_admin);
    }

    /// Step two, through the timelock itself: an operation calling each
    /// contract's `accept_admin`, executed after the delay.
    fn timelock_accepts(&self) -> soroban_sdk::BytesN<32> {
        let [m, o, t] = self.admin_contracts();
        let calls = self.p.accept_calls(&[&m, &o, &t]);
        let id = self.p.schedule(&self.tl, &calls, DELAY, None);
        self.p.warp(DELAY);
        self.tl.execute(&id);
        id
    }

    fn hand_off(&self) {
        self.propose_timelock(&self.tl.address);
        self.timelock_accepts();
    }

    fn assert_admins(&self, expected: &Address) {
        assert_eq!(self.p.market.get_admin(), *expected);
        assert_eq!(self.p.oracle.get_admin(), *expected);
        assert_eq!(self.treasury.get_admin(), *expected);
    }
}

#[test]
fn two_step_hand_off_moves_every_admin_to_the_timelock() {
    let w = world();
    w.propose_timelock(&w.tl.address);
    // Proposed, not moved: the deployer is still in charge.
    w.assert_admins(&w.p.admin);
    assert_eq!(w.p.market.get_pending_admin(), Some(w.tl.address.clone()));

    w.timelock_accepts();
    w.assert_admins(&w.tl.address);
    assert_eq!(w.p.market.get_pending_admin(), None);
    // The protocol vault's admin is still the market.
    assert_eq!(w.p.vault.get_admin(), w.p.market.address);
}

#[test]
fn an_admin_operation_is_rejected_before_the_delay_and_executed_after() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    let id = p.schedule(
        &w.tl,
        &[
            p.call(&p.market.address, "set_fee_rate", p.args1(120u32)),
            p.call(&p.oracle.address, "set_max_staleness", p.args1(900u64)),
            p.call(&w.treasury.address, "pause", Vec::new(&p.env)),
        ],
        DELAY,
        None,
    );
    assert!(w.tl.is_pending(&id));
    assert!(w.tl.try_execute(&id).is_err()); // NotReady
    p.warp(DELAY - 1);
    assert!(w.tl.try_execute(&id).is_err());
    assert_eq!(p.market.get_fee_rate(), 50);

    p.warp(1);
    assert!(w.tl.is_ready(&id));
    // Anyone can execute; no signature is involved.
    p.env.set_auths(&[]);
    w.tl.execute(&id);
    assert_eq!(p.market.get_fee_rate(), 120);
    assert_eq!(p.oracle.get_max_staleness(), 900);
    assert!(w.treasury.is_paused());
    assert!(w.tl.is_done(&id));
}

#[test]
fn a_direct_admin_call_after_hand_off_fails() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    p.env.set_auths(&[]);
    for who in [&p.admin, &w.proposer] {
        p.sign(
            who,
            &p.market.address,
            "set_fee_rate",
            (75u32,).into_val(&p.env),
        );
        assert!(p.market.try_set_fee_rate(&75).is_err());
        p.sign(
            who,
            &p.oracle.address,
            "set_min_reports",
            (3u32,).into_val(&p.env),
        );
        assert!(p.oracle.try_set_min_reports(&3).is_err());
        p.sign(who, &w.treasury.address, "pause", ().into_val(&p.env));
        assert!(w.treasury.try_pause().is_err());
        p.sign(
            who,
            &p.market.address,
            "transfer_admin",
            (who.clone(),).into_val(&p.env),
        );
        assert!(p.market.try_transfer_admin(who).is_err());
    }
    assert_eq!(p.market.get_fee_rate(), 50);
    assert_eq!(p.oracle.get_min_reports(), 2);
    assert!(!w.treasury.is_paused());
}

#[test]
fn cancellers_abort_pending_operations() {
    let w = world();
    w.hand_off();
    let p = &w.p;

    // The guardian (here also the proposer) cancels a bad operation.
    let attacker = Address::generate(&p.env);
    let bad = p.schedule(
        &w.tl,
        &[p.call(
            &p.market.address,
            "transfer_admin",
            p.args1(attacker.clone()),
        )],
        DELAY,
        None,
    );
    w.tl.cancel(&w.proposer, &bad);
    p.warp(DELAY);
    assert!(w.tl.try_execute(&bad).is_err());
    assert_eq!(p.market.get_pending_admin(), None);

    // A monitoring key is granted the canceller role (itself a delayed
    // operation) and aborts the next one.
    let monitor = Address::generate(&p.env);
    let grant = p.schedule(
        &w.tl,
        &[p.call(
            &w.tl.address,
            "set_canc",
            (monitor.clone(), true).into_val(&p.env),
        )],
        DELAY,
        None,
    );
    p.warp(DELAY);
    w.tl.execute(&grant);
    let bad = p.schedule(
        &w.tl,
        &[p.call(
            &p.oracle.address,
            "remove_feeder",
            p.args1(p.feeders[0].clone()),
        )],
        DELAY,
        None,
    );
    w.tl.cancel(&monitor, &bad);
    p.warp(DELAY);
    assert!(w.tl.try_execute(&bad).is_err());
    assert!(p.oracle.is_feeder(&p.feeders[0]));

    // Nobody else can cancel.
    let op = p.schedule(
        &w.tl,
        &[p.call(&p.market.address, "set_fee_rate", p.args1(60u32))],
        DELAY,
        None,
    );
    assert!(w.tl.try_cancel(&attacker, &op).is_err());
    assert!(w.tl.is_pending(&op));
}

#[test]
fn a_predecessor_dependency_is_honoured() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    // Re-point the oracle first, only then rely on it with a stricter
    // quorum — the second must never land without the first.
    let first = p.schedule(
        &w.tl,
        &[p.call(&p.market.address, "set_fee_rate", p.args1(120u32))],
        DELAY,
        None,
    );
    let second = p.schedule(
        &w.tl,
        &[p.call(&p.market.address, "set_fee_rate", p.args1(90u32))],
        DELAY,
        Some(&first),
    );
    p.warp(DELAY);
    assert!(!w.tl.is_ready(&second));
    assert!(w.tl.try_execute(&second).is_err()); // PredecessorNotDone
    w.tl.execute(&first);
    assert_eq!(p.market.get_fee_rate(), 120);
    w.tl.execute(&second);
    assert_eq!(p.market.get_fee_rate(), 90);
}

#[test]
fn handing_off_to_a_wrong_address_is_recoverable() {
    let w = world();
    let p = &w.p;
    // A typo'd / unusable admin address: nothing changes hands.
    let wrong = Address::generate(&p.env);
    w.propose_timelock(&wrong);
    w.assert_admins(&p.admin);
    p.env.set_auths(&[]);
    p.sign(
        &p.admin,
        &p.market.address,
        "set_fee_rate",
        (70u32,).into_val(&p.env),
    );
    p.market.set_fee_rate(&70);
    assert_eq!(p.market.get_fee_rate(), 70);

    // A timelock that accepts before anything was proposed to it fails
    // its whole operation — and changes nothing either.
    p.env.mock_all_auths();
    let premature = {
        let [m, o, t] = w.admin_contracts();
        p.schedule(&w.tl, &p.accept_calls(&[&m, &o, &t]), DELAY, None)
    };
    p.warp(DELAY);
    assert!(w.tl.try_execute(&premature).is_err());
    w.assert_admins(&p.admin);

    // The deployer simply re-proposes the right timelock.
    w.hand_off();
    w.assert_admins(&w.tl.address);
}

#[test]
fn a_misconfigured_timelock_never_receives_control() {
    let w = world();
    let p = &w.p;
    // A timelock whose proposer nobody controls can't schedule the
    // accept_admin calls, so the hand-off simply never completes.
    let nobody = Address::generate(&p.env);
    let stuck = p.timelock(&nobody, DELAY, GRACE);
    w.propose_timelock(&stuck.address);
    p.env.set_auths(&[]);
    let [m, o, t] = w.admin_contracts();
    let calls = p.accept_calls(&[&m, &o, &t]);
    let (mut targets, mut fns, mut args) = (Vec::new(&p.env), Vec::new(&p.env), Vec::new(&p.env));
    for (tgt, f, a) in calls {
        targets.push_back(tgt);
        fns.push_back(f);
        args.push_back(a);
    }
    let id = p.next_id();
    p.sign(
        &w.proposer,
        &stuck.address,
        "schedule",
        (
            id.clone(),
            targets.clone(),
            fns.clone(),
            args.clone(),
            DELAY,
        )
            .into_val(&p.env),
    );
    assert!(stuck
        .try_schedule(&id, &targets, &fns, &args, &DELAY)
        .is_err());
    w.assert_admins(&p.admin);

    // Recovery: propose the correctly configured timelock instead.
    p.env.mock_all_auths();
    w.hand_off();
    w.assert_admins(&w.tl.address);
}

#[test]
fn emergency_pause_stays_on_a_short_path() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    // The trusted multisig pauses immediately — no timelock delay.
    let id = p.next_id();
    p.approve(&id);
    p.market.pause_via_multisig(&p.multisig.address, &id);
    let id = p.next_id();
    p.approve(&id);
    p.oracle.pause_via_multisig(&p.multisig.address, &id);
    assert!(p.market.is_paused() && p.oracle.is_paused());

    // Pausing still lets positions wind down; unpausing is a normal
    // (delayed) admin operation.
    let op = p.schedule(
        &w.tl,
        &[
            p.call(&p.market.address, "unpause", Vec::new(&p.env)),
            p.call(&p.oracle.address, "unpause", Vec::new(&p.env)),
        ],
        DELAY,
        None,
    );
    p.warp(DELAY);
    w.tl.execute(&op);
    assert!(!p.market.is_paused() && !p.oracle.is_paused());
    let _ = UNIT;
}

#[test]
fn the_delay_is_measured_on_ledger_time_not_ledger_count() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    let op = p.schedule(
        &w.tl,
        &[p.call(&p.market.address, "set_fee_rate", p.args1(80u32))],
        DELAY,
        None,
    );
    // Twice the delay's worth of ~5s ledgers, but no time: still not
    // ready. The delay is a timestamp delay on both the timelock and
    // governor side, never a ledger count.
    let seq = p.env.ledger().sequence();
    p.env
        .ledger()
        .set_sequence_number(seq + 2 * (DELAY / 5) as u32);
    assert!(w.tl.try_execute(&op).is_err());
    p.warp(DELAY);
    w.tl.execute(&op);
    assert_eq!(p.market.get_fee_rate(), 80);
}

#[test]
fn the_proposer_can_take_admin_back_through_the_timelock() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    let [m, o, t] = w.admin_contracts();
    let back = w.proposer.clone();
    let op = p.schedule(
        &w.tl,
        &[
            p.call(&m, "transfer_admin", p.args1(back.clone())),
            p.call(&o, "transfer_admin", p.args1(back.clone())),
            p.call(&t, "transfer_admin", p.args1(back.clone())),
        ],
        DELAY,
        None,
    );
    p.warp(DELAY);
    w.tl.execute(&op);
    p.market.accept_admin();
    p.oracle.accept_admin();
    w.treasury.accept_admin();
    w.assert_admins(&back);
}

#[test]
fn an_expired_operation_cannot_run() {
    let w = world();
    w.hand_off();
    let p = &w.p;
    let op = p.schedule(
        &w.tl,
        &[p.call(&p.market.address, "set_fee_rate", p.args1(80u32))],
        DELAY,
        None,
    );
    p.warp(DELAY + GRACE + 1);
    assert!(w.tl.try_execute(&op).is_err());
    assert_eq!(p.market.get_fee_rate(), 50);
}
