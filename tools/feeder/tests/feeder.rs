use std::cell::{Cell, RefCell};
use std::time::Duration;

use zenith_feeder::config::Config;
use zenith_feeder::monitor::{Monitor, Status};
use zenith_feeder::report::{scale, Report};
use zenith_feeder::run_round;
use zenith_feeder::submit::{submit_with_retry, SubmitError, Submitter};

const CFG: &str = r#"{
  "rpc_url": "https://rpc.example.invalid",
  "network_passphrase": "Test SDF Network ; September 2015",
  "oracle_contract_id": "CORACLE",
  "symbols": [{"name":"XLM","decimals":7,"price":"0.1234"},{"name":"BTC","decimals":2,"price":"abc"}],
  "submit_interval_secs": 60,
  "staleness_window_secs": 300,
  "staleness_warn_secs": 180,
  "backoff_base_ms": 100
}"#;

struct Mock {
    fail_first: Cell<u32>,
    reject: bool,
    seen: RefCell<Vec<Report>>,
}

impl Mock {
    fn new(fail_first: u32, reject: bool) -> Self {
        Mock {
            fail_first: Cell::new(fail_first),
            reject,
            seen: RefCell::new(vec![]),
        }
    }
}

impl Submitter for Mock {
    fn submit(&self, r: &Report) -> Result<(), SubmitError> {
        if self.reject {
            return Err(SubmitError::Rejected("Error(Contract, #9)".into()));
        }
        if self.fail_first.get() > 0 {
            self.fail_first.set(self.fail_first.get() - 1);
            return Err(SubmitError::Transient("timeout".into()));
        }
        self.seen.borrow_mut().push(r.clone());
        Ok(())
    }
}

#[test]
fn config_validation() {
    assert!(Config::parse(CFG).is_ok());
    assert!(Config::parse(&CFG.replace(
        "\"staleness_warn_secs\": 180",
        "\"staleness_warn_secs\": 300"
    ))
    .is_err());
    assert!(Config::parse(&CFG.replace("https://", "ftp://")).is_err());
}

#[test]
fn price_scaling_respects_decimals() {
    assert_eq!(scale("0.1234", 7).unwrap(), 1_234_000);
    assert_eq!(scale("42", 2).unwrap(), 4200);
    assert!(scale("1.234", 2).is_err());
    assert!(scale("0", 2).is_err());
    assert!(scale("-1", 2).is_err());
}

#[test]
fn retries_back_off_exponentially_then_succeed() {
    let m = Mock::new(3, false);
    let delays = RefCell::new(vec![]);
    let r = Report {
        symbol: "XLM".into(),
        price: 1,
        decimals: 7,
        interval_id: 1,
    };
    submit_with_retry(&m, &r, 4, 100, &|d: Duration| {
        delays.borrow_mut().push(d.as_millis())
    })
    .unwrap();
    assert_eq!(*delays.borrow(), vec![100, 200, 400]);
}

#[test]
fn retries_are_bounded_and_rejections_not_retried() {
    let r = Report {
        symbol: "XLM".into(),
        price: 1,
        decimals: 7,
        interval_id: 1,
    };
    let n = Cell::new(0);
    assert!(submit_with_retry(&Mock::new(99, false), &r, 2, 1, &|_| n.set(n.get() + 1)).is_err());
    assert_eq!(n.get(), 2);
    n.set(0);
    assert!(matches!(
        submit_with_retry(&Mock::new(0, true), &r, 5, 1, &|_| n.set(n.get() + 1)),
        Err(SubmitError::Rejected(_))
    ));
    assert_eq!(n.get(), 0);
}

#[test]
fn round_submits_exact_payload_and_skips_bad_symbol() {
    let cfg = Config::parse(CFG).unwrap();
    let m = Mock::new(0, false);
    let mut mon = Monitor::new(1000);
    let st = run_round(&cfg, &m, &mut mon, 7, 1000, false, &|_| {});
    assert_eq!(st, Status::Ok);
    let seen = m.seen.borrow();
    assert_eq!(seen.len(), 1); // BTC "abc" skipped, XLM still reported
    assert_eq!(
        seen[0],
        Report {
            symbol: "XLM".into(),
            price: 1_234_000,
            decimals: 7,
            interval_id: 7
        }
    );
    // No secret ever enters a report payload.
    assert!(!seen[0].to_json().to_string().contains("S"));
}

#[test]
fn dry_run_submits_nothing() {
    let cfg = Config::parse(CFG).unwrap();
    let m = Mock::new(0, false);
    let mut mon = Monitor::new(0);
    run_round(&cfg, &m, &mut mon, 1, 0, true, &|_| {});
    assert!(m.seen.borrow().is_empty());
}

#[test]
fn staleness_thresholds() {
    let mut mon = Monitor::new(0);
    mon.record_accepted("XLM", 1000);
    assert_eq!(mon.check(1100, 300, 180), Status::Ok);
    assert_eq!(mon.check(1200, 300, 180), Status::Warn);
    assert!(matches!(mon.check(1400, 300, 180), Status::Exit(_)));
}

#[test]
fn contract_rejection_exits() {
    let cfg = Config::parse(CFG).unwrap();
    let mut mon = Monitor::new(0);
    let st = run_round(&cfg, &Mock::new(0, true), &mut mon, 1, 0, false, &|_| {});
    assert!(matches!(st, Status::Exit(_)));
}
