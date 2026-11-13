//! Table-driven scenarios for the restart state machine.
//!
//! Each test is one story a real deployment produces, with timestamps in ms.

use guard_core::{Event, Guard, Input, Reason, State, Timings};

fn guard() -> Guard {
    Guard::new(Timings::default())
}

fn events(out: &[guard_core::Output]) -> Vec<Event> {
    out.iter()
        .filter_map(|o| match o {
            guard_core::Output::Event(e) => Some(e.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn prior_signal_suppresses_immediately() {
    let mut g = guard();
    let out = g.step(
        Input::PriorSignal {
            reason: Reason::PriorSignal,
            window_ms: 60_000,
        },
        0,
    );
    assert_eq!(g.state(), State::Planned);
    assert!(g.suppression());
    assert!(out.contains(&guard_core::Output::StateChanged(State::Planned)));
    assert!(out.contains(&guard_core::Output::Suppression(true)));
}

#[test]
fn planned_restart_cycle_ends_with_one_suppressed_event() {
    let mut g = guard();
    g.step(
        Input::PriorSignal {
            reason: Reason::PriorSignal,
            window_ms: 120_000,
        },
        0,
    );
    g.step(Input::BridgeOffline, 1_000);
    let out = g.step(Input::BridgeOnline, 10_000);

    assert_eq!(g.state(), State::Settling);
    assert!(g.suppression());
    let ev = events(&out);
    assert_eq!(
        ev,
        vec![Event::RestartSuppressed {
            reason: Reason::PriorSignal,
            downtime_ms: 9_000
        }]
    );

    g.step(Input::Tick, 30_000);
    assert_eq!(g.state(), State::Idle);
    assert!(!g.suppression());
}

#[test]
fn unexplained_offline_suspects_and_suppresses_during_grace() {
    let mut g = guard();
    let out = g.step(Input::BridgeOffline, 0);
    assert_eq!(g.state(), State::Suspect);
    assert!(g.suppression());
    assert!(events(&out).is_empty());
    assert!(!g.is_attributed());
}

#[test]
fn unexplained_outage_escalates_after_grace() {
    let mut g = guard();
    g.step(Input::BridgeOffline, 0);
    let out = g.step(Input::Tick, 15_000);

    assert_eq!(g.state(), State::OutageConfirmed);
    assert!(!g.suppression(), "a real outage must never stay suppressed");
    assert_eq!(
        events(&out),
        vec![Event::OutageConfirmed { offline_ms: 15_000 }]
    );
}

#[test]
fn recovery_within_grace_settles_without_outage_event() {
    let mut g = guard();
    g.step(Input::BridgeOffline, 0);
    g.step(Input::BridgeOnline, 5_000);
    assert_eq!(g.state(), State::Settling);

    let out = g.step(Input::Tick, 25_000);
    assert_eq!(g.state(), State::Idle);
    assert!(!g.suppression());
    assert!(!events(&out)
        .iter()
        .any(|e| matches!(e, Event::OutageConfirmed { .. })));
}

#[test]
fn clean_container_exit_makes_the_restart_planned() {
    let mut g = guard();
    g.step(Input::ContainerDie { exit_code: 0 }, 0);
    g.step(Input::BridgeOffline, 5_000);

    assert_eq!(g.state(), State::Planned);
    assert_eq!(g.attribution(), Some(Reason::ComposeRecreate));
    assert!(g.suppression());
}

#[test]
fn crash_exit_code_escalates_instead_of_being_planned() {
    let mut g = guard();
    g.step(Input::ContainerDie { exit_code: 137 }, 0);
    g.step(Input::BridgeOffline, 5_000);
    assert_eq!(g.state(), State::Suspect);
    assert_eq!(g.attribution(), Some(Reason::Crash));

    g.step(Input::Tick, 20_000);
    assert_eq!(g.state(), State::OutageConfirmed);
    assert!(!g.suppression());
}

#[test]
fn stale_container_exit_is_not_trusted() {
    let mut g = guard();
    g.step(Input::ContainerDie { exit_code: 0 }, 0);
    g.step(Input::BridgeOffline, 40_000); // 40s > 30s attribution window

    assert_eq!(g.state(), State::Suspect);
    assert_eq!(g.attribution(), None);
}

#[test]
fn prior_signal_without_restart_releases_after_its_window() {
    let mut g = guard();
    g.step(
        Input::PriorSignal {
            reason: Reason::PriorSignal,
            window_ms: 120_000,
        },
        0,
    );
    assert!(g.suppression());

    g.step(Input::Tick, 120_000);
    assert_eq!(g.state(), State::Idle);
    assert!(!g.suppression());
}

#[test]
fn offline_while_settling_counts_as_a_new_outage() {
    let mut g = guard();
    g.step(Input::ContainerDie { exit_code: 0 }, 0);
    g.step(Input::BridgeOffline, 5_000);
    g.step(Input::BridgeOnline, 15_000);
    assert_eq!(g.state(), State::Settling);

    g.step(Input::BridgeOffline, 20_000);
    assert_eq!(
        g.state(),
        State::Suspect,
        "the earlier die event must not cover a second outage"
    );
}

#[test]
fn outage_confirmed_then_back_online_settles_and_clears() {
    let mut g = guard();
    g.step(Input::BridgeOffline, 0);
    g.step(Input::Tick, 15_000);
    assert_eq!(g.state(), State::OutageConfirmed);

    g.step(Input::BridgeOnline, 20_000);
    assert_eq!(g.state(), State::Settling);
    assert!(g.suppression());

    g.step(Input::Tick, 40_000);
    assert_eq!(g.state(), State::Idle);
    assert!(!g.suppression());
}

#[test]
fn spurious_online_while_idle_is_a_noop() {
    let mut g = guard();
    let out = g.step(Input::BridgeOnline, 0);
    assert_eq!(g.state(), State::Idle);
    assert!(!g.suppression());
    assert!(out.is_empty());
}

#[test]
fn suppression_is_only_announced_when_it_changes() {
    let mut g = guard();
    let out = g.step(Input::BridgeOffline, 0);
    let announces = |o: &[guard_core::Output]| {
        o.iter()
            .filter(|x| matches!(x, guard_core::Output::Suppression(_)))
            .count()
    };
    assert_eq!(announces(&out), 1);

    let out = g.step(Input::BridgeOffline, 2_000);
    assert_eq!(announces(&out), 0, "repeat must not re-announce");
}
