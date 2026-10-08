//! Recorded sessions replayed through the follower: each must end with the items its
//! `.expected` file lists and never show a message twice. `SESH_BLESS=1` rewrites the files.

use std::path::Path;

use sesh_transcript::replay::replay;

#[test]
fn recorded_sessions_replay_to_their_expected_items() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sessions");
    let mut recordings: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.path()).collect();
    recordings.retain(|path| path.extension().is_some_and(|ext| ext == "jsonl"));
    recordings.sort();
    assert!(!recordings.is_empty(), "no recordings in {}", dir.display());
    for recording in recordings {
        let work = std::env::temp_dir().join(format!("sesh-replay-test-{}", std::process::id()));
        let outcome = replay(&recording, &work).unwrap();
        let _ = std::fs::remove_dir_all(&work);
        let name = recording.display();
        assert_eq!(outcome.doubles, Vec::<String>::new(), "{name} showed a message twice");
        assert!(outcome.provisional > 0, "{name} never showed a row from the screen");
        let mut sessions: Vec<_> = outcome.items.into_iter().collect();
        sessions.sort();
        let got: String = sessions.iter().flat_map(|(_, items)| items.iter().map(|item| format!("{}\n", serde_json::to_string(item).unwrap()))).collect();
        let expected = recording.with_extension("expected");
        if std::env::var_os("SESH_BLESS").is_some() {
            std::fs::write(&expected, &got).unwrap();
        }
        assert_eq!(got, std::fs::read_to_string(&expected).unwrap_or_default(), "{name} ended with other items");
    }
}
