//! Tests for `MissionMemoryIndex` — tokenization, TF/IDF scoring, and
//! on-disk index persistence.

use super::*;
use std::path::Path;

use crate::test_helpers::temp_dir;

fn write_fixture_mission(
    root: &Path,
    mission_id: &str,
    title: &str,
    template: &str,
    file_paths: &[&str],
    summaries: &[&str],
) {
    let mdir = root.join(".nit").join("swarm").join(mission_id);
    std::fs::create_dir_all(&mdir).unwrap();

    let run = serde_json::json!({
        "id": mission_id,
        "title": title,
        "template": template,
        "status": "DONE",
        "updated_at": "t+0",
        "tasks": [],
    });
    std::fs::write(mdir.join("run.json"), serde_json::to_string(&run).unwrap()).unwrap();

    let summaries_arr: Vec<_> = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| serde_json::json!({"summary": s, "task_id": format!("t-{i:03}")}))
        .collect();
    let summary = serde_json::json!({
        "mission_id": mission_id,
        "summaries": summaries_arr,
    });
    std::fs::write(
        mdir.join("summary.json"),
        serde_json::to_string(&summary).unwrap(),
    )
    .unwrap();

    let tdir = mdir.join("tasks").join("t-001");
    std::fs::create_dir_all(&tdir).unwrap();
    let files_arr: Vec<_> = file_paths
        .iter()
        .map(|p| serde_json::json!({"path": p}))
        .collect();
    let artifacts = serde_json::json!({ "files": files_arr });
    std::fs::write(
        tdir.join("artifacts.json"),
        serde_json::to_string(&artifacts).unwrap(),
    )
    .unwrap();
}

#[test]
fn tokenize_strips_stopwords_and_lowercases() {
    let toks = tokenize("The Quick Brown Fox");
    assert!(toks.contains(&"quick".to_string()));
    assert!(toks.contains(&"brown".to_string()));
    assert!(toks.contains(&"fox".to_string()));
    assert!(!toks.contains(&"the".to_string()));
}

#[test]
fn tokenize_splits_snake_case() {
    let toks = tokenize("some_thing_here");
    assert!(toks.contains(&"some".to_string()));
    assert!(toks.contains(&"thing".to_string()));
    assert!(toks.contains(&"here".to_string()));
}

#[test]
fn path_tokens_splits_paths() {
    let toks = path_tokens(&["crates/nit-gol/src/catalog.rs".to_string()]);
    assert!(toks.contains(&"crates".to_string()));
    assert!(toks.contains(&"nit-gol".to_string()));
    assert!(toks.contains(&"src".to_string()));
    assert!(toks.contains(&"catalog".to_string()));
}

#[test]
fn build_index_from_corpus_fixture() {
    let root = temp_dir("mm-build");
    write_fixture_mission(
        &root,
        "mis-001",
        "refactor nit-gol module",
        "parallel",
        &["crates/nit-gol/src/catalog.rs"],
        &["File-by-file refactor of nit-gol"],
    );
    write_fixture_mission(
        &root,
        "mis-002",
        "audit orchestration docs",
        "bulk",
        &["docs/orchestration.md"],
        &["Reviewed all orchestration documentation"],
    );

    let idx = build_index(&root);

    assert_eq!(idx.missions.len(), 2);
    let titles: Vec<&str> = idx.missions.iter().map(|m| m.title.as_str()).collect();
    assert!(titles.contains(&"refactor nit-gol module"));
    assert!(titles.contains(&"audit orchestration docs"));
    for m in &idx.missions {
        assert!(!m.tags.is_empty(), "mission {} has no tags", m.mission_id);
    }
}

#[test]
fn build_index_skips_empty_mission_dirs() {
    let root = temp_dir("mm-skip");
    // Empty mission directory (no summary.json, no run.json, no tasks).
    std::fs::create_dir_all(root.join(".nit").join("swarm").join("mis-099")).unwrap();
    // Non-mission directory (doesn't start with mis-).
    std::fs::create_dir_all(root.join(".nit").join("swarm").join("foo-abc")).unwrap();
    write_fixture_mission(
        &root,
        "mis-001",
        "real mission",
        "parallel",
        &["src/lib.rs"],
        &["did something"],
    );

    let idx = build_index(&root);

    assert_eq!(idx.missions.len(), 1);
    assert_eq!(idx.missions[0].mission_id, "mis-001");
}

#[test]
fn retrieve_returns_expected_ordering() {
    let root = temp_dir("mm-order");
    write_fixture_mission(
        &root,
        "mis-001",
        "refactor catalog module",
        "parallel",
        &["crates/nit-gol/src/catalog.rs"],
        &["Heavy catalog refactor with dedupe"],
    );
    write_fixture_mission(
        &root,
        "mis-002",
        "audit docs",
        "bulk",
        &["docs/catalog.md"],
        &["Touched docs only"],
    );
    write_fixture_mission(
        &root,
        "mis-003",
        "completely unrelated frontend work",
        "parallel",
        &["ui/button.tsx"],
        &["Button restyle"],
    );

    let idx = build_index(&root);
    let hits = retrieve_similar(&idx, "refactor catalog module with dedupe", &[], &[], 5);

    assert!(hits.len() >= 2);
    assert_eq!(hits[0].mission.mission_id, "mis-001");
    // Second hit is the weakly-matching docs mission.
    assert_eq!(hits[1].mission.mission_id, "mis-002");
    assert!(hits.iter().all(|h| h.mission.mission_id != "mis-003"));
}

#[test]
fn retrieve_empty_corpus_returns_empty() {
    let idx = MissionMemoryIndex::default();
    let hits = retrieve_similar(&idx, "anything", &[], &[], 5);
    assert!(hits.is_empty());
}

#[test]
fn retrieve_path_bonus_boosts_file_overlap() {
    let root = temp_dir("mm-path");
    write_fixture_mission(
        &root,
        "mis-001",
        "generic work",
        "parallel",
        &["crates/nit-gol/src/catalog.rs"],
        &["refactor"],
    );
    write_fixture_mission(
        &root,
        "mis-002",
        "generic work",
        "parallel",
        &["ui/button.tsx"],
        &["refactor"],
    );

    let idx = build_index(&root);
    let scope_tokens = path_tokens(&["crates/nit-gol/src/catalog.rs".to_string()]);
    let hits = retrieve_similar(&idx, "refactor", &scope_tokens, &[], 5);

    assert!(!hits.is_empty());
    assert_eq!(hits[0].mission.mission_id, "mis-001");
}

#[test]
fn retrieve_excludes_listed_missions() {
    let root = temp_dir("mm-exclude");
    write_fixture_mission(
        &root,
        "mis-001",
        "refactor catalog module",
        "parallel",
        &["crates/nit-gol/src/catalog.rs"],
        &["Heavy catalog refactor"],
    );
    write_fixture_mission(
        &root,
        "mis-002",
        "refactor catalog module again",
        "parallel",
        &["crates/nit-gol/src/catalog.rs"],
        &["Another catalog refactor"],
    );

    let idx = build_index(&root);
    let hits = retrieve_similar(&idx, "refactor catalog", &[], &["mis-001"], 5);

    assert!(hits.iter().all(|h| h.mission.mission_id != "mis-001"));
    assert!(hits.iter().any(|h| h.mission.mission_id == "mis-002"));
}

#[test]
fn upsert_mission_dedupes_by_id() {
    let root = temp_dir("mm-upsert");
    write_fixture_mission(&root, "mis-001", "first", "parallel", &["a/b.rs"], &["one"]);

    let idx1 = upsert_mission(&root, "mis-001").unwrap();
    let idx2 = upsert_mission(&root, "mis-001").unwrap();

    assert_eq!(idx1.missions.len(), 1);
    assert_eq!(idx2.missions.len(), 1);
}

#[test]
fn load_tolerates_corrupt_json() {
    let root = temp_dir("mm-corrupt");
    let memdir = root.join(".nit").join("memory");
    std::fs::create_dir_all(&memdir).unwrap();
    std::fs::write(memdir.join("index.json"), b"{{{ not valid json").unwrap();

    let idx = load_index(&root);

    assert!(idx.missions.is_empty());
}

#[test]
fn save_and_load_round_trip() {
    let root = temp_dir("mm-roundtrip");
    write_fixture_mission(
        &root,
        "mis-001",
        "roundtrip mission",
        "parallel",
        &["src/lib.rs"],
        &["did work"],
    );

    let built = build_index(&root);
    save_index(&root, &built).unwrap();
    let loaded = load_index(&root);

    assert_eq!(built, loaded);
}

#[test]
fn idf_weight_decreases_with_frequency() {
    // Rare terms (df=1 of 4) outweigh ubiquitous ones (df=4 of 4).
    let rare = idf_weight(1, 4);
    let common = idf_weight(4, 4);
    assert!(
        rare > common,
        "rare term should weigh more than common; rare={rare}, common={common}",
    );
    // Empty corpus and never-seen terms both fall back to the neutral weight
    // of 1.0; retrieval never assigns negative weight.
    assert_eq!(idf_weight(0, 0), 1.0);
    assert_eq!(idf_weight(0, 10), 1.0);
}

#[test]
fn idf_weight_prefers_rare_matches() {
    // Corpus where "refactor" appears in every mission (df=3/3) and
    // "lifehash" only appears in mis-001 (df=1/3). Plain Jaccard would tie
    // common/rare matches; IDF weighting must rank the rare match higher.
    let root = temp_dir("mm-idf-rare-wins");
    write_fixture_mission(
        &root,
        "mis-001",
        "refactor lifehash encoder",
        "parallel",
        &["crates/nit-core/src/lifehash.rs"],
        &["restructured lifehash"],
    );
    write_fixture_mission(
        &root,
        "mis-002",
        "refactor agent console",
        "parallel",
        &["crates/nit-tui/src/widgets/agent_console_view.rs"],
        &["cleaned up console"],
    );
    write_fixture_mission(
        &root,
        "mis-003",
        "refactor genome report",
        "parallel",
        &["crates/nit-core/src/genome_report.rs"],
        &["tuned report"],
    );

    let idx = build_index(&root);
    let hits = retrieve_similar(&idx, "lifehash refactor", &[], &[], 5);

    assert!(!hits.is_empty());
    assert_eq!(hits[0].mission.mission_id, "mis-001");
    if hits.len() >= 2 {
        assert!(
            hits[0].score > hits[1].score + 0.01,
            "rare-term match should clearly outrank common-term matches; \
             got {:?} vs {:?}",
            hits[0].score,
            hits[1].score,
        );
    }
}
