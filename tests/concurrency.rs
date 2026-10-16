mod common;

use std::collections::HashSet;
use std::thread;

use common::Repo;

/// Two creates at the same time: each either wins a fresh id, or loses cleanly. The state is never corrupt.
#[test]
fn parallel_creates_never_corrupt_or_duplicate() {
    let r = Repo::new();
    let mut wins = 0;
    let mut losses = 0;
    for round in 0..20 {
        r.write(&format!("round{round}.txt"), "x\n");
        let results: Vec<_> = thread::scope(|s| {
            let handles: Vec<_> = (0..2)
                .map(|n| {
                    let r = &r;
                    s.spawn(move || r.stone(&[&format!("round {round} writer {n}")]))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for o in &results {
            match o.code {
                0 => wins += 1,
                // Lost the race, or saw the other's stone and had nothing new to add.
                1 => {
                    losses += 1;
                    assert!(
                        o.stderr.contains(
                            "Another git stone command changed the stones at the same time"
                        ) || o.stderr.contains("Nothing changed since Stone"),
                        "unexpected refusal: {}",
                        o.stderr
                    );
                }
                other => panic!("exit {other}: {}{}", o.stdout, o.stderr),
            }
        }
        assert!(
            results.iter().any(|o| o.code == 0),
            "round {round}: nobody won"
        );

        // The state loads, ids are unique and increasing, and every stone has its snapshot.
        let list = r.stone(&["list", "--porcelain"]);
        assert_eq!(list.code, 0, "{}", list.stderr);
        let ids: Vec<u64> = list
            .stdout
            .split('\0')
            .filter(|rec| rec.starts_with("stone\t"))
            .map(|rec| rec.split('\t').nth(1).unwrap().parse().unwrap())
            .collect();
        assert_eq!(
            ids.iter().collect::<HashSet<_>>().len(),
            ids.len(),
            "duplicate ids: {ids:?}"
        );
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
        assert_eq!(ids.len(), wins, "one stone per success");
    }
    r.git(&["fsck", "--no-dangling"]);
    assert!(wins >= 20 && wins + losses == 40);
}
