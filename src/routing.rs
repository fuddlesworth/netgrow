use std::collections::HashSet;

use rand::Rng;

pub fn route_link<R: Rng>(
    from: (i16, i16),
    to: (i16, i16),
    occupied: &HashSet<(i16, i16)>,
    bounds: (i16, i16),
    rng: &mut R,
) -> Option<Vec<(i16, i16)>> {
    let horizontal_first = rng.gen_bool(0.5);
    let jog = rng.gen_bool(0.4);

    let variants = [
        (horizontal_first, jog),
        (!horizontal_first, jog),
        (horizontal_first, false),
        (!horizontal_first, false),
    ];

    for (hfirst, do_jog) in variants {
        if let Some(path) = build_path(from, to, hfirst, do_jog, rng) {
            if path_clear(&path, from, to, occupied, bounds) {
                return Some(path);
            }
        }
    }
    None
}

fn build_path<R: Rng>(
    from: (i16, i16),
    to: (i16, i16),
    horizontal_first: bool,
    jog: bool,
    rng: &mut R,
) -> Option<Vec<(i16, i16)>> {
    let pivot = if horizontal_first {
        (to.0, from.1)
    } else {
        (from.0, to.1)
    };

    let mut pts: Vec<(i16, i16)> = Vec::new();
    pts.push(from);

    // Insert a small perpendicular "shoulder" jog entirely within the first
    // leg. The bump must end before the pivot column/row so the second leg
    // never overlaps a previously visited cell — that overlap was the source
    // of the renderer's broken corner glyphs.
    if jog {
        let offset: i16 = if rng.gen_bool(0.5) { 1 } else { -1 };
        if horizontal_first {
            let leg_min = from.0.min(pivot.0);
            let leg_max = from.0.max(pivot.0);
            let leg_len = leg_max - leg_min;
            if leg_len >= 5 {
                let q = (leg_len / 4).max(1);
                let going_right = pivot.0 > from.0;
                let (x1, x2) = if going_right {
                    (from.0 + q, pivot.0 - q)
                } else {
                    (from.0 - q, pivot.0 + q)
                };
                let in_range = if going_right { x2 > x1 } else { x2 < x1 };
                if in_range {
                    pts.push((x1, from.1));
                    pts.push((x1, from.1 + offset));
                    pts.push((x2, from.1 + offset));
                    pts.push((x2, from.1));
                }
            }
        } else {
            let leg_min = from.1.min(pivot.1);
            let leg_max = from.1.max(pivot.1);
            let leg_len = leg_max - leg_min;
            if leg_len >= 5 {
                let q = (leg_len / 4).max(1);
                let going_down = pivot.1 > from.1;
                let (y1, y2) = if going_down {
                    (from.1 + q, pivot.1 - q)
                } else {
                    (from.1 - q, pivot.1 + q)
                };
                let in_range = if going_down { y2 > y1 } else { y2 < y1 };
                if in_range {
                    pts.push((from.0, y1));
                    pts.push((from.0 + offset, y1));
                    pts.push((from.0 + offset, y2));
                    pts.push((from.0, y2));
                }
            }
        }
    }

    pts.push(pivot);
    pts.push(to);

    let mut out = Vec::new();
    out.push(from);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        if a.0 != b.0 && a.1 != b.1 {
            // Not axis-aligned — reject.
            return None;
        }
        for cell in step(a, b).skip(1) {
            out.push(cell);
        }
    }
    // Dedupe consecutive duplicates.
    out.dedup();
    Some(out)
}

pub fn step(a: (i16, i16), b: (i16, i16)) -> Box<dyn Iterator<Item = (i16, i16)>> {
    if a.0 == b.0 {
        let (y0, y1) = (a.1, b.1);
        let range: Box<dyn Iterator<Item = i16>> = if y0 <= y1 {
            Box::new(y0..=y1)
        } else {
            Box::new((y1..=y0).rev())
        };
        let x = a.0;
        Box::new(range.map(move |y| (x, y)))
    } else if a.1 == b.1 {
        let (x0, x1) = (a.0, b.0);
        let range: Box<dyn Iterator<Item = i16>> = if x0 <= x1 {
            Box::new(x0..=x1)
        } else {
            Box::new((x1..=x0).rev())
        };
        let y = a.1;
        Box::new(range.map(move |x| (x, y)))
    } else {
        Box::new(std::iter::empty())
    }
}

fn path_clear(
    path: &[(i16, i16)],
    from: (i16, i16),
    to: (i16, i16),
    occupied: &HashSet<(i16, i16)>,
    bounds: (i16, i16),
) -> bool {
    for &c in path {
        if c.0 < 0 || c.1 < 0 || c.0 >= bounds.0 || c.1 >= bounds.1 {
            return false;
        }
        if c == from || c == to {
            continue;
        }
        if occupied.contains(&c) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_chacha::ChaCha8Rng;
    use rand::SeedableRng;

    #[test]
    fn straight_horizontal_path() {
        let occ = HashSet::new();
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let p = route_link((2, 5), (8, 5), &occ, (40, 20), &mut rng).unwrap();
        assert_eq!(*p.first().unwrap(), (2, 5));
        assert_eq!(*p.last().unwrap(), (8, 5));
        assert!(p.len() >= 7);
    }

    #[test]
    fn l_path_has_bend() {
        let occ = HashSet::new();
        let mut rng = ChaCha8Rng::seed_from_u64(2);
        let p = route_link((2, 2), (8, 6), &occ, (40, 20), &mut rng).unwrap();
        assert_eq!(*p.first().unwrap(), (2, 2));
        assert_eq!(*p.last().unwrap(), (8, 6));
    }

    #[test]
    fn jog_path_has_no_duplicate_cells() {
        // Probe many seeds and many endpoint shapes to catch the regression
        // where the jog branch produced a back-and-forth path that visited
        // the same cell twice and broke renderer corner glyphs.
        for seed in 0..200u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let occ = HashSet::new();
            for &(from, to) in &[
                ((5, 5), (20, 15)),
                ((20, 15), (5, 5)),
                ((10, 10), (10, 25)),
                ((10, 10), (25, 10)),
                ((30, 5), (5, 25)),
                ((5, 5), (25, 7)),
            ] {
                if let Some(path) = route_link(from, to, &occ, (60, 40), &mut rng) {
                    let unique: HashSet<(i16, i16)> = path.iter().copied().collect();
                    assert_eq!(
                        unique.len(),
                        path.len(),
                        "duplicate cells in path seed={} from={:?} to={:?}: {:?}",
                        seed,
                        from,
                        to,
                        path
                    );
                    // Also verify each step is exactly 1 Chebyshev cell.
                    for w in path.windows(2) {
                        let dx = (w[1].0 - w[0].0).abs();
                        let dy = (w[1].1 - w[0].1).abs();
                        assert!(
                            (dx == 1 && dy == 0) || (dx == 0 && dy == 1),
                            "non-unit step seed={} from={:?} to={:?}: {:?} -> {:?}",
                            seed,
                            from,
                            to,
                            w[0],
                            w[1]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn collision_forces_fallback() {
        let mut occ = HashSet::new();
        // Block the (x=to.x, y=from.y) pivot path by filling column 8.
        for y in 0..10 {
            occ.insert((8, y));
        }
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let _ = route_link((2, 2), (10, 6), &occ, (40, 20), &mut rng);
    }
}
