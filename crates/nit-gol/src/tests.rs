use crate::{analyze::evaluate_rule, grid::EdgeMode, step::step, Grid, Rule};

#[test]
fn parse_rule_roundtrip() {
    let rule = Rule::parse("B36/S23").expect("parse");
    assert_eq!(rule.to_string(), "B36/S23");
}

#[test]
fn block_still_life() {
    let mut grid = Grid::new(4, 4);
    grid.set(1, 1, true);
    grid.set(1, 2, true);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    let rule = Rule::conway();
    let next = step(&grid, rule, EdgeMode::Dead);
    assert_eq!(grid, next);
}

#[test]
fn blinker_oscillator() {
    let mut grid = Grid::new(5, 5);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    grid.set(2, 3, true);
    let rule = Rule::conway();
    let next = step(&grid, rule, EdgeMode::Dead);
    assert!(next.get(1, 2));
    assert!(next.get(2, 2));
    assert!(next.get(3, 2));
    let next2 = step(&next, rule, EdgeMode::Dead);
    assert_eq!(grid, next2);
}

#[test]
fn evaluate_rule_detects_period() {
    let mut grid = Grid::new(5, 5);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    grid.set(2, 3, true);
    let rule = Rule::conway();
    let eval = evaluate_rule(&grid, rule, EdgeMode::Dead, 10);
    assert_eq!(eval.period, Some(2));
    assert!(eval.score > 0.0);
}
