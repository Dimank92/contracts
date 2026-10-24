#![cfg(test)]

//! Event schema v2 (#126): every lp_pool event conforms to events.json
//! and carries the documented data.

extern crate std;

use crate::test::{MockMarket, MockMarketClient};
use crate::{
    ClaimedEvent, CollateralReclaimedEvent, DepositQueuedEvent, EpochProcessedEvent,
    InitializedEvent, KeeperSetEvent, LiabilitiesMarkedEvent, LpPool, LpPoolClient,
    MaxUtilizationSetEvent, OptionWrittenEvent, SeriesCapSetEvent, WithdrawQueuedEvent,
    VIRTUAL_SHARES,
};
use event_spec::{events_named, Recorder};
use soroban_sdk::{testutils::Address as _, token, Address, Env};

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, keeper) = (Address::generate(&env), Address::generate(&env));
    let usdc = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let market = MockMarketClient::new(&env, &env.register_contract(None, MockMarket));
    market.init(&usdc, &500);
    token::StellarAssetClient::new(&env, &usdc).mint(&market.address, &1_000_000);
    let user = Address::generate(&env);
    token::StellarAssetClient::new(&env, &usdc).mint(&user, &100_000);
    let pool = LpPoolClient::new(&env, &env.register_contract(None, LpPool));
    let mut rec = Recorder::new("lp_pool", &pool.address);
    let at = &pool.address;

    pool.initialize(&admin, &keeper, &usdc, &market.address, &8_000);
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, at, "initialized"),
        [InitializedEvent {
            admin,
            keeper: keeper.clone(),
            asset: usdc,
            market: market.address.clone(),
            max_utilization_bps: 8_000,
        }]
    );

    pool.set_keeper(&keeper);
    pool.set_max_utilization(&9_000);
    pool.set_series_cap(&1, &50_000);
    rec.record(&env);
    assert_eq!(
        events_named::<KeeperSetEvent>(&env, at, "keeper_set"),
        [KeeperSetEvent { keeper }]
    );
    assert_eq!(
        events_named::<MaxUtilizationSetEvent>(&env, at, "max_utilization_set"),
        [MaxUtilizationSetEvent {
            max_utilization_bps: 9_000
        }]
    );
    assert_eq!(
        events_named::<SeriesCapSetEvent>(&env, at, "series_cap_set"),
        [SeriesCapSetEvent {
            max_collateral: 50_000
        }]
    );

    pool.deposit(&user, &100_000);
    rec.record(&env);
    assert_eq!(
        events_named::<DepositQueuedEvent>(&env, at, "deposit_queued"),
        [DepositQueuedEvent {
            amount: 100_000,
            ticket: 100_000,
            pending_deposits: 100_000,
        }]
    );

    pool.process_epoch();
    rec.record(&env);
    let shares = 100_000 * VIRTUAL_SHARES;
    pool.claim(&user);
    rec.record(&env);

    let position = pool.write_option(&1, &1, &40_000, &0);
    rec.record(&env);
    assert_eq!(
        events_named::<OptionWrittenEvent>(&env, at, "option_written"),
        [OptionWrittenEvent {
            collateral: 40_000,
            premium: 500,
            idle: 60_500,
            locked: 40_000,
        }]
    );

    pool.set_expected_liabilities(&1_000);
    rec.record(&env);
    assert_eq!(
        events_named::<LiabilitiesMarkedEvent>(&env, at, "liabilities_marked"),
        [LiabilitiesMarkedEvent {
            expected_liabilities: 1_000
        }]
    );

    market.settle(&position, &39_000);
    pool.reclaim(&position);
    rec.record(&env);
    assert_eq!(
        events_named::<CollateralReclaimedEvent>(&env, at, "collateral_reclaimed"),
        [CollateralReclaimedEvent {
            series_id: 1,
            collateral: 40_000,
            returned: 39_000,
            idle: 99_500,
            locked: 0,
        }]
    );

    pool.request_withdraw(&user, &shares);
    rec.record(&env);
    assert_eq!(
        events_named::<WithdrawQueuedEvent>(&env, at, "withdraw_queued"),
        [WithdrawQueuedEvent {
            shares,
            ticket: shares,
            share_balance: 0,
            pending_withdraw_shares: shares,
        }]
    );

    let result = pool.process_epoch();
    rec.record(&env);
    let processed = events_named::<EpochProcessedEvent>(&env, at, "epoch_processed");
    assert_eq!(
        processed[0],
        EpochProcessedEvent {
            total_assets: 0,
            total_shares: 0,
            deposit_assets: 100_000,
            deposit_shares: shares,
            withdraw_shares: 0,
            withdraw_assets: 0,
            idle: 100_000,
            total_shares_after: shares,
        }
    );
    assert_eq!(
        processed[1],
        EpochProcessedEvent {
            total_assets: 99_500,
            total_shares: shares,
            deposit_assets: 0,
            deposit_shares: 0,
            withdraw_shares: shares,
            withdraw_assets: result.withdraw_assets,
            idle: 99_500 - result.withdraw_assets,
            total_shares_after: 0,
        }
    );

    pool.claim(&user);
    rec.record(&env);
    assert_eq!(
        events_named::<ClaimedEvent>(&env, at, "claimed"),
        [
            ClaimedEvent {
                shares,
                assets: 0,
                share_balance: shares,
            },
            ClaimedEvent {
                shares: 0,
                assets: result.withdraw_assets,
                share_balance: 0,
            },
        ]
    );

    rec.assert_complete();
}
