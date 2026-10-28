#![no_std]

//! Zenith Streams — linear, per-second token streams with an optional
//! cliff, funded upfront by the funder (typically the treasury).
//!
//! `streamed(t) = total × (t − start) / (end − start)`, floored, clamped to
//! `[0, total]` and zero before `cliff`. Because every intermediate value
//! is floored and the recipient can only ever withdraw
//! `streamed(now) − withdrawn`, the total paid out can never exceed the
//! deposit (see the property tests in test.rs).
//!
//! Funding operations (`top_up`, `extend_duration`, `increase_rate`) rebase
//! the schedule at a checkpoint `(anchor, base)`: what is vested at the
//! anchor is frozen into `base` and only the remainder is re-spread, so the
//! recipient is never owed less than before the call. Dust from the integer
//! division when extending `end` is floored, which rounds the new rate up
//! (never down) in the recipient's favour.
//!
//! The recipient withdraws at any time and may transfer the stream to a
//! new recipient. The funder or governance can cancel: the recipient is
//! paid everything accrued but not yet withdrawn, and the funder is
//! refunded the unstreamed remainder.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Vec};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod events;
pub mod math;
mod migrations;
mod types;

use error::Error;
pub use events::{
    InitializedEvent, StreamCanceledEvent, StreamCreatedEvent, StreamTransferredEvent,
    WithdrawnEvent,
};
pub use migrations::CURRENT_SCHEMA_VERSION;
pub use types::{DataKey, Schedule, Stream, MAX_CHECKPOINTS};

#[contract]
pub struct Streams;

fn load(env: &Env, stream_id: u64) -> Stream {
    env.storage()
        .persistent()
        .get(&DataKey::Stream(stream_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::StreamNotFound))
}

fn save(env: &Env, stream_id: u64, stream: &Stream) {
    env.storage()
        .persistent()
        .set(&DataKey::Stream(stream_id), stream);
}

fn checkpoint(env: &Env, stream_id: u64, stream: &Stream) -> (u64, i128) {
    env.storage()
        .persistent()
        .get(&DataKey::Checkpoint(stream_id))
        .unwrap_or((stream.start, 0))
}

/// Continuous linear accrual from checkpoint or `start`, floored.
fn linear_or_checkpointed(env: &Env, stream_id: u64, stream: &Stream, now: u64) -> i128 {
    let (anchor, base) = checkpoint(env, stream_id, stream);
    if now <= anchor {
        return base;
    }
    if stream.end <= anchor {
        return stream.total;
    }
    base + math::mul_div_floor(
        stream.total - base,
        (now - anchor) as i128,
        (stream.end - anchor) as i128,
    )
}

/// Amount streamed to the recipient as of `now`, in `[0, total]`.
fn streamed_at(env: &Env, stream_id: u64, stream: &Stream, now: u64) -> i128 {
    if stream.canceled || now >= stream.end {
        return stream.total;
    }
    withdrawable_at(env, stream_id, stream, now)
}

/// Pure function of the ledger timestamp and the schedule: the cumulative
/// amount vested as of `now`, in `[0, total]`. Never decreases with time.
fn withdrawable_at(env: &Env, stream_id: u64, stream: &Stream, now: u64) -> i128 {
    if stream.canceled || now >= stream.end {
        return stream.total;
    }
    if now < stream.cliff {
        return 0;
    }
    match &stream.schedule {
        Schedule::Linear => linear_or_checkpointed(env, stream_id, stream, now),
        Schedule::LinearWithCliff(cliff) => {
            if now < *cliff {
                0
            } else {
                linear_or_checkpointed(env, stream_id, stream, now)
            }
        }
        Schedule::Stepped(checkpoints) => {
            let mut vested = 0i128;
            for (ts, cumulative) in checkpoints.iter() {
                if now >= ts {
                    vested = cumulative;
                } else {
                    break;
                }
            }
            vested
        }
    }
}

/// Loads a stream that can still be funded: not cancelled, not yet ended.
fn load_fundable(env: &Env, caller: &Address, stream_id: u64) -> Stream {
    caller.require_auth();
    migrations::require_current(env);
    let stream = load(env, stream_id);
    if *caller != stream.funder
        && Some(caller.clone())
            != env
                .storage()
                .persistent()
                .get::<_, Address>(&DataKey::Delegate(stream_id))
    {
        panic_with_error!(env, Error::Unauthorized);
    }
    if stream.canceled {
        panic_with_error!(env, Error::StreamCanceled);
    }
    if env.ledger().timestamp() >= stream.end || stream.withdrawn == stream.total {
        panic_with_error!(env, Error::StreamCompleted);
    }
    stream
}

/// Freezes what has vested by `max(now, start)` into the checkpoint and
/// returns `(anchor, vested, remaining_total)`.
fn rebase(env: &Env, stream_id: u64, stream: &Stream) -> (u64, i128, i128) {
    let anchor = env.ledger().timestamp().max(stream.start);
    let vested = streamed_at(env, stream_id, stream, anchor);
    env.storage()
        .persistent()
        .set(&DataKey::Checkpoint(stream_id), &(anchor, vested));
    (anchor, vested, stream.total - vested)
}

/// Pulls `amount` from `from` and reverts with `TransferShortfall` unless
/// the contract's balance grew by at least that much, so a fee-on-transfer,
/// no-op or lying token can never leave a stream credited for tokens that
/// did not arrive.
fn pull(env: &Env, token: &Address, from: &Address, amount: i128) {
    let client = token::Client::new(env, token);
    let this = env.current_contract_address();
    let before = client.balance(&this);
    client.transfer(from, &this, &amount);
    match client.balance(&this).checked_sub(before) {
        Some(received) if received >= amount => {}
        _ => panic_with_error!(env, Error::TransferShortfall),
    }
}

/// Validates a schedule against the stream bounds and total, panicking with
/// a typed error on any violation.
fn validate_schedule(env: &Env, schedule: &Schedule, total: i128, start: u64, end: u64) {
    match schedule {
        Schedule::Linear => {}
        Schedule::LinearWithCliff(cliff) => {
            if *cliff < start || *cliff > end {
                panic_with_error!(env, Error::InvalidSchedule);
            }
        }
        Schedule::Stepped(checkpoints) => {
            if checkpoints.len() > MAX_CHECKPOINTS {
                panic_with_error!(env, Error::TooManyCheckpoints);
            }
            if checkpoints.is_empty() {
                panic_with_error!(env, Error::InvalidSchedule);
            }
            let mut first = true;
            let mut prev_ts = 0u64;
            let mut prev_cum = 0i128;
            for (ts, cumulative) in checkpoints.iter() {
                if first {
                    if ts < start || cumulative < 0 || ts > end || cumulative > total {
                        panic_with_error!(env, Error::InvalidSchedule);
                    }
                    first = false;
                } else if ts <= prev_ts || cumulative < prev_cum || ts > end || cumulative > total {
                    panic_with_error!(env, Error::InvalidSchedule);
                }
                prev_ts = ts;
                prev_cum = cumulative;
            }
            if prev_cum != total {
                panic_with_error!(env, Error::InvalidSchedule);
            }
        }
    }
}

#[contractimpl]
impl Streams {
    pub fn initialize(env: Env, governance: Address) {
        if env.storage().instance().has(&DataKey::Governance) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::StreamCounter, &0u64);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
        events::initialized(
            &env,
            InitializedEvent {
                governance,
                schema_version: CURRENT_SCHEMA_VERSION,
            },
        );
    }

    pub fn get_governance(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Governance).unwrap()
    }

    /// Creates a stream and pulls `total` of `token` from `funder` upfront.
    /// `start` may be in the past (the elapsed portion is withdrawable
    /// immediately, subject to the cliff). Zero-duration streams
    /// (`start == end`) are rejected.
    ///
    /// Backward-compatible entrypoint: builds a `LinearWithCliff` schedule
    /// (or plain `Linear` when `cliff == start`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
    ) -> u64 {
        let schedule = if cliff == start {
            Schedule::Linear
        } else {
            Schedule::LinearWithCliff(cliff)
        };
        Self::create_stream_with_schedule(
            env, funder, recipient, token, total, start, cliff, end, schedule,
        )
    }

    /// Creates a stream with a stepped schedule.
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream_stepped(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
        checkpoints: Vec<(u64, i128)>,
    ) -> u64 {
        Self::create_stream_with_schedule(
            env,
            funder,
            recipient,
            token,
            total,
            start,
            cliff,
            end,
            Schedule::Stepped(checkpoints),
        )
    }

    /// Creates a stream with an explicit schedule variant, selected at
    /// creation time. The schedule is validated once here rather than on
    /// every read.
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream_with_schedule(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
        schedule: Schedule,
    ) -> u64 {
        funder.require_auth();
        migrations::require_current(&env);
        if total <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if start >= end || cliff < start || cliff > end {
            panic_with_error!(&env, Error::InvalidSchedule);
        }
        validate_schedule(&env, &schedule, total, start, end);

        pull(&env, &token, &funder, total);

        let stream_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .unwrap();
        env.storage()
            .instance()
            .set(&DataKey::StreamCounter, &(stream_id + 1));

        save(
            &env,
            stream_id,
            &Stream {
                funder: funder.clone(),
                recipient: recipient.clone(),
                token: token.clone(),
                total,
                start,
                cliff,
                end,
                schedule,
                withdrawn: 0,
                canceled: false,
            },
        );
        events::created(
            &env,
            stream_id,
            StreamCreatedEvent {
                funder,
                recipient,
                token,
                total,
                start,
                cliff,
                end,
            },
        );
        stream_id
    }

    /// Recipient withdraws up to their currently accrued balance.
    pub fn withdraw(env: Env, stream_id: u64, amount: i128) {
        let mut stream = load(&env, stream_id);
        stream.recipient.require_auth();
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let available =
            streamed_at(&env, stream_id, &stream, env.ledger().timestamp()) - stream.withdrawn;
        if amount > available {
            panic_with_error!(&env, Error::InsufficientBalance);
        }
        stream.withdrawn += amount;
        save(&env, stream_id, &stream);

        token::Client::new(&env, &stream.token).transfer(
            &env.current_contract_address(),
            &stream.recipient,
            &amount,
        );
        events::withdrawn(
            &env,
            stream_id,
            WithdrawnEvent {
                recipient: stream.recipient.clone(),
                amount,
                withdrawn: stream.withdrawn,
            },
        );
    }

    /// Lets `delegate` (or nobody, with `None`) call funding entrypoints.
    pub fn set_funding_delegate(env: Env, stream_id: u64, delegate: Option<Address>) {
        let stream = load(&env, stream_id);
        stream.funder.require_auth();
        migrations::require_current(&env);
        let key = DataKey::Delegate(stream_id);
        match delegate {
            Some(addr) => env.storage().persistent().set(&key, &addr),
            None => env.storage().persistent().remove(&key),
        }
    }

    /// Tops up a running stream with `amount` of additional tokens, pulling
    /// them from `caller` and extending `end` so the per-second rate is
    /// preserved (rounded in the recipient's favour).
    pub fn top_up(env: Env, caller: Address, stream_id: u64, amount: i128) {
        let mut stream = load_fundable(&env, &caller, stream_id);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let (anchor, _, remaining) = rebase(&env, stream_id, &stream);
        let remaining_duration = stream
            .end
            .checked_sub(anchor)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Overflow));
        let added_duration = (amount as u128)
            .checked_mul(remaining_duration as u128)
            .and_then(|p| p.checked_div(remaining as u128))
            .and_then(|d| u64::try_from(d).ok())
            .unwrap_or_else(|| panic_with_error!(&env, Error::Overflow));
        let new_end = stream
            .end
            .checked_add(added_duration)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Overflow));

        pull(&env, &stream.token, &caller, amount);
        stream.total += amount;
        stream.end = new_end;
        save(&env, stream_id, &stream);
        events::topped_up(&env, stream_id, caller, amount, new_end);
    }

    /// Pushes `end` out by `extra_time` seconds without adding funds.
    /// The unvested remainder is re-spread over the longer duration.
    pub fn extend_duration(env: Env, caller: Address, stream_id: u64, extra_time: u64) {
        let mut stream = load_fundable(&env, &caller, stream_id);
        if extra_time == 0 {
            panic_with_error!(&env, Error::InvalidSchedule);
        }
        let new_end = stream
            .end
            .checked_add(extra_time)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Overflow));
        let _ = rebase(&env, stream_id, &stream);
        stream.end = new_end;
        save(&env, stream_id, &stream);
        events::duration_extended(&env, stream_id, new_end);
    }

    /// Raises the rate to `new_rate` tokens per second over the remaining
    /// duration, pulling the difference from `caller`. Must exceed the
    /// current rate (`remaining ÷ duration`).
    pub fn increase_rate(env: Env, caller: Address, stream_id: u64, new_rate: i128) {
        let mut stream = load_fundable(&env, &caller, stream_id);
        let (anchor, _, remaining) = rebase(&env, stream_id, &stream);
        let needed = new_rate
            .checked_mul((stream.end - anchor) as i128)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Overflow));
        if new_rate <= 0 || needed <= remaining {
            panic_with_error!(&env, Error::InvalidRate);
        }
        let added = needed - remaining;

        pull(&env, &stream.token, &caller, added);
        stream.total += added;
        save(&env, stream_id, &stream);
        events::rate_changed(&env, stream_id, caller, added, new_rate);
    }

    /// Funder or governance cancels. The recipient receives everything
    /// accrued but not yet withdrawn (zero if still before the cliff) and
    /// the funder is refunded the unstreamed remainder.
    pub fn cancel(env: Env, caller: Address, stream_id: u64) {
        caller.require_auth();
        migrations::require_current(&env);
        let mut stream = load(&env, stream_id);
        if caller != stream.funder && caller != Self::get_governance(env.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }

        let streamed = streamed_at(&env, stream_id, &stream, env.ledger().timestamp());
        let to_recipient = streamed - stream.withdrawn;
        let to_funder = stream.total - streamed;

        stream.total = streamed;
        stream.withdrawn = streamed;
        stream.canceled = true;
        save(&env, stream_id, &stream);

        let token = token::Client::new(&env, &stream.token);
        let this = env.current_contract_address();
        if to_recipient > 0 {
            token.transfer(&this, &stream.recipient, &to_recipient);
        }
        if to_funder > 0 {
            token.transfer(&this, &stream.funder, &to_funder);
        }
        events::canceled(
            &env,
            stream_id,
            StreamCanceledEvent {
                caller,
                to_recipient,
                to_funder,
                streamed,
            },
        );
    }

    /// Recipient hands the stream (including any accrued, unwithdrawn
    /// balance) to a new recipient.
    pub fn transfer(env: Env, stream_id: u64, new_recipient: Address) {
        let mut stream = load(&env, stream_id);
        stream.recipient.require_auth();
        migrations::require_current(&env);
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }
        let old = stream.recipient.clone();
        stream.recipient = new_recipient.clone();
        save(&env, stream_id, &stream);
        events::transferred(
            &env,
            stream_id,
            StreamTransferredEvent {
                from: old,
                to: new_recipient,
            },
        );
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Governance only. Returns
    /// the version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, Error> {
        Self::get_governance(env.clone()).require_auth();
        zenith_common::migrations::migrate(
            &env,
            from,
            to,
            CURRENT_SCHEMA_VERSION,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> u32 {
        zenith_common::migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        zenith_common::migrations::migration_state(&env)
    }

    /// `(recipient_withdrawable, funder_refundable)` as of now: what the
    /// recipient could withdraw right now, and what the funder would get
    /// back if the stream were cancelled right now.
    pub fn balance_of(env: Env, stream_id: u64) -> (i128, i128) {
        let stream = load(&env, stream_id);
        let streamed = streamed_at(&env, stream_id, &stream, env.ledger().timestamp());
        (streamed - stream.withdrawn, stream.total - streamed)
    }

    /// Cumulative amount vested as of now, per the stream's schedule. Pure
    /// function of the ledger timestamp and the schedule.
    pub fn withdrawable(env: Env, stream_id: u64) -> i128 {
        let stream = load(&env, stream_id);
        withdrawable_at(&env, stream_id, &stream, env.ledger().timestamp())
    }

    pub fn get_stream(env: Env, stream_id: u64) -> Option<Stream> {
        env.storage().persistent().get(&DataKey::Stream(stream_id))
    }

    pub fn get_stream_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .unwrap()
    }
}

/// Convenience constructor for a `Stepped` schedule from parallel arrays.
pub fn stepped_schedule(checkpoints: Vec<(u64, i128)>) -> Schedule {
    Schedule::Stepped(checkpoints)
}
