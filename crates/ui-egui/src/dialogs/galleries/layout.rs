//! The order of a gallery's photos and chapters, and what a drag, a selection click or a filter does to it.
//! Pure functions on photo keys (`pid`): the manager draws and sends, this decides.

/// A photo of the gallery in display order: its key and its chapter (`""` = none).
pub type Flat = Vec<(String, String)>;

/// Where dragged photos land.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    /// Just before this photo, in its chapter.
    Before(String),
    /// Just after this photo, in its chapter.
    After(String),
    /// At the end of this chapter (created when no photo is in it yet; `""` = no chapter).
    End(String),
}

/// The photos in the order the gallery's page shows them: chapters first in the order the owner chose
/// (`order`, named chapters only), the others by first appearance; inside a chapter, the photos' order.
/// `photos` is in position order, as the server lists it.
pub fn display(photos: &[(String, String)], order: &[String]) -> Flat {
    let mut names: Vec<&str> = order.iter().map(String::as_str).filter(|n| photos.iter().any(|(_, s)| s == n)).collect();
    for (_, s) in photos {
        if !names.contains(&s.as_str()) {
            names.push(s);
        }
    }
    names.iter().flat_map(|n| photos.iter().filter(move |(_, s)| s == n).cloned()).collect()
}

/// The chapters of a display order, as (name, photo keys); runs of one chapter are always contiguous.
pub fn chapters(flat: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (pid, section) in flat {
        match out.iter_mut().find(|(n, _)| n == section) {
            Some((_, v)) => v.push(pid.clone()),
            None => out.push((section.clone(), vec![pid.clone()])),
        }
    }
    out
}

/// `flat` with `moved` taken out and put at `to`, taking the chapter of the place they land in. The moved photos
/// keep their order among themselves. `None` when nothing would change (dropped on themselves, nothing to move).
pub fn moved(flat: &[(String, String)], moved: &[String], to: &Drop) -> Option<Flat> {
    let carried: Flat = flat.iter().filter(|(p, _)| moved.contains(p)).cloned().collect();
    if carried.is_empty() {
        return None;
    }
    let rest: Flat = flat.iter().filter(|(p, _)| !moved.contains(p)).cloned().collect();
    let at = |pid: &String| rest.iter().position(|(p, _)| p == pid);
    let (index, section) = match to {
        Drop::Before(t) => {
            let i = at(t)?;
            (i, rest[i].1.clone())
        }
        Drop::After(t) => {
            let i = at(t)?;
            (i + 1, rest[i].1.clone())
        }
        Drop::End(name) => (rest.iter().rposition(|(_, s)| s == name).map_or(rest.len(), |i| i + 1), name.clone()),
    };
    let mut out = rest;
    out.splice(index..index, carried.into_iter().map(|(p, _)| (p, section.clone())));
    // the chapters' display order must survive: regroup, and say there was no change when nothing moved
    let out = display(&out, &first_appearance(&out));
    (out != flat).then_some(out)
}

/// The named chapters of `flat` by first appearance: the order to send so a chapter does not jump elsewhere.
pub fn first_appearance(flat: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, s) in flat {
        if !s.is_empty() && !names.contains(s) {
            names.push(s.clone());
        }
    }
    names
}

/// A click on a tile: Cmd/Ctrl toggles it, Shift takes the range from the last click, a plain click picks it alone.
/// `visible` is what is on screen, in order; `anchor` is the last tile clicked.
pub fn click(picked: &mut Vec<String>, anchor: &mut Option<String>, visible: &[String], pid: &str, command: bool, shift: bool) {
    let here = visible.iter().position(|p| p == pid);
    let from = anchor.as_ref().and_then(|a| visible.iter().position(|p| p == a));
    if shift && let (Some(a), Some(b)) = (from, here) {
        let (lo, hi) = (a.min(b), a.max(b));
        let range: Vec<String> = visible[lo..=hi].to_vec();
        if command {
            for p in range {
                if !picked.contains(&p) {
                    picked.push(p);
                }
            }
        } else {
            *picked = range;
        }
        return;
    }
    if command {
        match picked.iter().position(|p| p == pid) {
            Some(i) => {
                picked.remove(i);
            }
            None => picked.push(pid.to_string()),
        }
    } else {
        *picked = vec![pid.to_string()];
    }
    *anchor = Some(pid.to_string());
}

/// The keys to carry when a tile starts a drag: the whole selection when the tile is part of it, else the tile alone.
pub fn carried(picked: &[String], pid: &str) -> Vec<String> {
    if picked.iter().any(|p| p == pid) { picked.to_vec() } else { vec![pid.to_string()] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(items: &[(&str, &str)]) -> Flat {
        items.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect()
    }
    fn pids(flat: &Flat) -> String {
        flat.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join("")
    }
    fn secs(flat: &Flat) -> String {
        flat.iter().map(|(_, s)| if s.is_empty() { "-" } else { s.as_str() }).collect::<Vec<_>>().join("")
    }
    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn display_follows_the_chosen_chapter_order_then_first_appearance() {
        let photos = f(&[("a", "B"), ("b", "A"), ("c", ""), ("d", "B"), ("e", "C")]);
        // no order chosen: first appearance, the unnamed group where it first shows
        assert_eq!(pids(&display(&photos, &[])), "adbce");
        // chosen: C first (a chapter that no longer exists is ignored), the rest as they appear
        let shown = display(&photos, &v(&["C", "Gone", "A"]));
        assert_eq!((pids(&shown), secs(&shown)), ("ebadc".to_string(), "CABB-".to_string()));
        assert_eq!(chapters(&shown).iter().map(|(n, p)| format!("{n}:{}", p.join(""))).collect::<Vec<_>>(), ["C:e", "A:b", "B:ad", ":c"]);
    }

    #[test]
    fn moved_photos_take_the_chapter_of_where_they_land_and_keep_their_order() {
        let flat = f(&[("a", "X"), ("b", "X"), ("c", "Y"), ("d", "Y"), ("e", "")]);
        // two photos from different chapters, dropped before d: they join Y, in their own order
        let out = moved(&flat, &v(&["e", "a"]), &Drop::Before("d".into())).expect("moves");
        assert_eq!((pids(&out), secs(&out)), ("bcaed".to_string(), "XYYYY".to_string()));
        // after the last photo of a chapter stays in that chapter
        let out = moved(&flat, &v(&["a"]), &Drop::After("b".into())).expect("moves");
        assert_eq!(pids(&out), "bacde");
        // reordering inside a chapter
        let out = moved(&flat, &v(&["b"]), &Drop::Before("a".into())).expect("moves");
        assert_eq!(pids(&out), "bacde");
        assert_eq!(secs(&out), "XXYY-");
    }

    #[test]
    fn dropping_in_place_or_on_oneself_changes_nothing() {
        let flat = f(&[("a", "X"), ("b", "X"), ("c", "Y")]);
        assert_eq!(moved(&flat, &v(&["a"]), &Drop::Before("b".into())), None, "already just before b");
        assert_eq!(moved(&flat, &v(&["b"]), &Drop::After("a".into())), None);
        assert_eq!(moved(&flat, &v(&["a"]), &Drop::Before("a".into())), None, "on itself");
        assert_eq!(moved(&flat, &v(&["a", "b"]), &Drop::After("b".into())), None, "on the group");
        assert_eq!(moved(&flat, &v(&["zz"]), &Drop::End("Y".into())), None, "unknown photo");
        assert_eq!(moved(&flat, &v(&["a"]), &Drop::Before("zz".into())), None, "unknown target");
    }

    #[test]
    fn end_of_a_chapter_and_a_new_chapter() {
        let flat = f(&[("a", "X"), ("b", "Y"), ("c", "Y"), ("d", "")]);
        let out = moved(&flat, &v(&["a"]), &Drop::End("Y".into())).expect("moves");
        assert_eq!((pids(&out), secs(&out)), ("bcad".to_string(), "YYY-".to_string()));
        // a chapter that does not exist yet starts at the end; the chapters keep their order
        let out = moved(&flat, &v(&["b", "d"]), &Drop::End("Z".into())).expect("moves");
        assert_eq!((pids(&out), secs(&out)), ("acbd".to_string(), "XYZZ".to_string()));
        assert_eq!(first_appearance(&out), v(&["X", "Y", "Z"]));
        // out of every chapter
        let out = moved(&flat, &v(&["b"]), &Drop::End(String::new())).expect("moves");
        assert_eq!((pids(&out), secs(&out)), ("acdb".to_string(), "XY--".to_string()));
    }

    #[test]
    fn a_click_picks_toggles_or_takes_a_range() {
        let visible = v(&["a", "b", "c", "d", "e"]);
        let (mut picked, mut anchor) = (Vec::new(), None);
        click(&mut picked, &mut anchor, &visible, "b", false, false);
        assert_eq!(picked, v(&["b"]));
        click(&mut picked, &mut anchor, &visible, "d", false, true);
        assert_eq!(picked, v(&["b", "c", "d"]), "range from the anchor");
        click(&mut picked, &mut anchor, &visible, "a", true, false);
        assert_eq!(picked, v(&["b", "c", "d", "a"]), "cmd adds");
        click(&mut picked, &mut anchor, &visible, "c", true, false);
        assert_eq!(picked, v(&["b", "d", "a"]), "cmd takes away");
        click(&mut picked, &mut anchor, &visible, "e", false, false);
        assert_eq!(picked, v(&["e"]), "plain click replaces");
        // shift with no anchor on screen acts as a plain click
        let mut anchor = Some("gone".to_string());
        click(&mut picked, &mut anchor, &visible, "c", false, true);
        assert_eq!((picked, anchor), (v(&["c"]), Some("c".to_string())));
    }

    #[test]
    fn a_drag_carries_the_selection_only_from_inside_it() {
        assert_eq!(carried(&v(&["a", "b"]), "b"), v(&["a", "b"]));
        assert_eq!(carried(&v(&["a", "b"]), "z"), v(&["z"]));
        assert_eq!(carried(&[], "z"), v(&["z"]));
    }
}
