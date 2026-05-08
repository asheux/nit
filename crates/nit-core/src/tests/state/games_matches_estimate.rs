//! `estimate_total_matches` parity with the runtime tournament kernel, plus
//! the FSM raw-count probe that documents why we need a CPU cap at all.

use super::*;

#[test]
fn fsm_family_raw_count_can_exceed_legacy_machine_cap() {
    let raw_count = nit_games::fsm_count(4, 2).expect("expected finite raw FSM count");
    assert!(raw_count > MAX_FAMILY_RUN_MACHINES_CPU as u128);
}

#[test]
fn estimate_total_matches_matches_runtime_schedule_math() {
    for strategy_count in [1usize, 2, 3, 7] {
        for repetitions in [1u32, 2, 5] {
            for self_play in [false, true] {
                let estimated = estimate_total_matches(strategy_count, repetitions, self_play)
                    .expect("estimate should not overflow");

                let mut src = format!(
                    "schema_version = 1\ngame = \"ipd\"\nrounds = 2\nrepetitions = {repetitions}\nself_play = {self_play}\n\n"
                );
                for idx in 0..strategy_count {
                    src.push_str(&format!(
                        "[[strategy]]\nid = \"fsm_{idx}\"\ntype = \"fsm\"\nnum_states = 1\nstart_state = 0\noutputs = [\"C\"]\ntransitions = [[0, 0]]\n\n"
                    ));
                }

                let cfg = nit_games::config::GamesConfig::from_toml(&src)
                    .expect("runtime config should parse");
                let runtime = nit_games::TournamentKernel::new(cfg).total_matches() as u128;
                assert_eq!(
                    estimated, runtime,
                    "mismatch for strategy_count={strategy_count}, repetitions={repetitions}, self_play={self_play}"
                );
            }
        }
    }
}
