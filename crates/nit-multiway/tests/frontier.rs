use nit_multiway::frontier::Frontier;
use nit_multiway::node::NodeId;

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

#[test]
fn empty_frontier_has_no_best() {
    let mut frontier = Frontier::default();
    assert!(frontier.is_empty());
    assert_eq!(frontier.len(), 0);
    assert_eq!(frontier.peek_best(), None);
    assert_eq!(frontier.pop_best(), None);
}

#[test]
fn pops_highest_score_first() {
    let mut frontier = Frontier::default();
    frontier.push(id("low"), 0.10);
    frontier.push(id("high"), 0.90);
    frontier.push(id("mid"), 0.50);

    assert_eq!(frontier.pop_best(), Some(id("high")));
    assert_eq!(frontier.pop_best(), Some(id("mid")));
    assert_eq!(frontier.pop_best(), Some(id("low")));
    assert_eq!(frontier.pop_best(), None);
}

#[test]
fn peek_best_does_not_consume() {
    let mut frontier = Frontier::default();
    frontier.push(id("a"), 0.25);
    frontier.push(id("b"), 0.75);

    assert_eq!(frontier.peek_best(), Some(&id("b")));
    assert_eq!(frontier.len(), 2);
    assert_eq!(frontier.pop_best(), Some(id("b")));
}

#[test]
fn equal_scores_break_ties_deterministically() {
    let mut frontier = Frontier::default();
    frontier.push(id("beta"), 0.5);
    frontier.push(id("alpha"), 0.5);

    // Tie resolved on NodeId: the lexicographically smaller id is drawn first.
    assert_eq!(frontier.pop_best(), Some(id("alpha")));
    assert_eq!(frontier.pop_best(), Some(id("beta")));
}

#[test]
fn nan_score_does_not_panic_the_heap() {
    let mut frontier = Frontier::default();
    frontier.push(id("a"), 0.3);
    frontier.push(id("nan"), f32::NAN);
    frontier.push(id("b"), 0.7);

    // total_cmp gives a total order, so a degenerate NaN never panics ordering.
    assert_eq!(frontier.len(), 3);
    assert!(frontier.pop_best().is_some());
    assert!(frontier.pop_best().is_some());
    assert!(frontier.pop_best().is_some());
    assert_eq!(frontier.pop_best(), None);
}
