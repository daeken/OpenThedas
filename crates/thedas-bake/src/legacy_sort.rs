//! The MSVC sort used by CreateAtlas.exe (RVA 0x6cf0) and by EclipseRay's
//! lightmap camera (RVA 0xc4d0). The partition order affects their output.

pub(crate) fn sort_by<T: Copy>(order: &mut [T], precedes: impl Fn(T, T) -> bool) {
    introsort(order, order.len(), &precedes);
}

fn introsort<T: Copy>(mut order: &mut [T], mut ideal: usize, precedes: &impl Fn(T, T) -> bool) {
    while order.len() > 32 {
        if ideal == 0 {
            heap_sort(order, precedes);
            return;
        }
        let (lower, upper) = partition(order, precedes);
        let (before, rest) = order.split_at_mut(lower);
        let after = &mut rest[upper - lower..];
        ideal = ideal / 2 + ideal / 4;
        // Recurse into the smaller range to limit stack growth.
        if before.len() < after.len() {
            introsort(before, ideal, precedes);
            order = after;
        } else {
            introsort(after, ideal, precedes);
            order = before;
        }
    }
    for index in 1..order.len() {
        let item = order[index];
        // The original tests the first item before walking the range.
        // The camera's tolerance comparison can make those paths disagree.
        if precedes(item, order[0]) {
            order.copy_within(..index, 1);
            order[0] = item;
            continue;
        }
        let mut hole = index;
        while hole > 1 && precedes(item, order[hole - 1]) {
            order[hole] = order[hole - 1];
            hole -= 1;
        }
        order[hole] = item;
    }
}

fn median<T: Copy>(
    order: &mut [T],
    first: usize,
    middle: usize,
    last: usize,
    precedes: &impl Fn(T, T) -> bool,
) {
    for (left, right) in [(first, middle), (middle, last), (first, middle)] {
        if precedes(order[right], order[left]) {
            order.swap(left, right);
        }
    }
}

fn partition<T: Copy>(order: &mut [T], precedes: &impl Fn(T, T) -> bool) -> (usize, usize) {
    let middle = order.len() / 2;
    let last = order.len() - 1;
    if last > 40 {
        let step = order.len() / 8;
        median(order, 0, step, 2 * step, precedes);
        median(order, middle - step, middle, middle + step, precedes);
        median(order, last - 2 * step, last - step, last, precedes);
        median(order, step, middle, last - step, precedes);
    } else {
        median(order, 0, middle, last, precedes);
    }
    // The comparisons use the current start of the partition, including
    // when an equivalent item changes that boundary (EclipseRay RVA 0xb6c0).
    let equivalent = |a, b| !precedes(a, b) && !precedes(b, a);
    let mut lower = middle;
    let mut upper = middle + 1;
    while lower > 0 && equivalent(order[lower - 1], order[lower]) {
        lower -= 1;
    }
    while upper < order.len() && equivalent(order[upper], order[lower]) {
        upper += 1;
    }

    let mut left = lower;
    let mut right = upper;
    loop {
        while right < order.len() {
            if !precedes(order[lower], order[right]) {
                if precedes(order[right], order[lower]) {
                    break;
                }
                order.swap(upper, right);
                upper += 1;
            }
            right += 1;
        }
        while left > 0 {
            left -= 1;
            if !precedes(order[left], order[lower]) {
                if precedes(order[lower], order[left]) {
                    left += 1;
                    break;
                }
                lower -= 1;
                order.swap(lower, left);
            }
        }
        match (left == 0, right == order.len()) {
            (true, true) => return (lower, upper),
            (true, false) => {
                if upper != right {
                    order.swap(lower, upper);
                }
                order.swap(lower, right);
                lower += 1;
                upper += 1;
                right += 1;
            }
            (false, true) => {
                left -= 1;
                lower -= 1;
                upper -= 1;
                order.swap(left, lower);
                order.swap(lower, upper);
            }
            (false, false) => {
                left -= 1;
                order.swap(left, right);
                right += 1;
            }
        }
    }
}

fn heap_sort<T: Copy>(order: &mut [T], precedes: &impl Fn(T, T) -> bool) {
    for root in (0..order.len() / 2).rev() {
        fill_heap_hole(order, root, order[root], precedes);
    }
    for end in (1..order.len()).rev() {
        let displaced = order[end];
        order[end] = order[0];
        fill_heap_hole(&mut order[..end], 0, displaced, precedes);
    }
}

fn fill_heap_hole<T: Copy>(
    order: &mut [T],
    root: usize,
    item: T,
    precedes: &impl Fn(T, T) -> bool,
) {
    let mut hole = root;
    while hole < order.len() / 2 {
        let left = 2 * hole + 1;
        let right = left + 1;
        let child = if right < order.len() && !precedes(order[right], order[left]) {
            right
        } else {
            left
        };
        order[hole] = order[child];
        hole = child;
    }
    while hole > root {
        let parent = (hole - 1) / 2;
        if !precedes(order[parent], item) {
            break;
        }
        order[hole] = order[parent];
        hole = parent;
    }
    order[hole] = item;
}

#[cfg(test)]
mod tests {
    use super::sort_by;

    #[test]
    fn camera_insertion_compares_with_first_boundary() {
        let edges = [(1, false), (0, false), (4, false), (2, true)];
        let mut order = [0, 1, 2, 3];
        sort_by(&mut order, |a, b| {
            let (height_a, has_triangle) = edges[a];
            let (height_b, _) = edges[b];
            if i32::abs(height_a - height_b) <= 1 {
                has_triangle
            } else {
                height_a < height_b
            }
        });
        // The original camera sort processes the two close lower edges
        // after the triangle's upper edge, in their input order.
        assert_eq!(order, [3, 0, 1, 2]);
    }
}
