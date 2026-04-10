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

    if jog && ((from.0 - to.0).abs() + (from.1 - to.1).abs()) >= 4 {
        // Insert a small perpendicular jog along the first leg.
        let mid = midpoint(from, pivot);
        let offset: i16 = if rng.gen_bool(0.5) { 1 } else { -1 };
        let jog_a = if horizontal_first {
            (mid.0, mid.1 + offset)
        } else {
            (mid.0 + offset, mid.1)
        };
        let jog_b = if horizontal_first {
            (pivot.0.min(to.0).max(from.0.min(to.0)), mid.1 + offset)
        } else {
            (mid.0 + offset, pivot.1.min(to.1).max(from.1.min(to.1)))
        };
        pts.push(mid);
        pts.push(jog_a);
        pts.push(jog_b);
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

fn midpoint(a: (i16, i16), b: (i16, i16)) -> (i16, i16) {
    ((a.0 + b.0) / 2, (a.1 + b.1) / 2)
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
