use std::collections::BTreeMap;

use crate::ContextEntry;

/// A grouped set of entries that belong to the same derived session.
#[derive(Debug, Clone)]
pub struct SessionGroup {
    /// Derived or explicit session identifier.
    pub session_id: String,
    /// Owned entries for this session group.
    pub entries: Vec<ContextEntry>,
}

/// Groups entries by explicit session id, then by fallback timestamp proximity.
#[must_use]
pub fn group_entries_by_session(
    entries: &[ContextEntry],
    proximity_threshold_secs: i64,
) -> Vec<SessionGroup> {
    assert!(
        proximity_threshold_secs >= 0,
        "proximity_threshold_secs must be non-negative"
    );

    let mut explicit_groups: BTreeMap<String, Vec<ContextEntry>> = BTreeMap::new();
    let mut fallback_entries: Vec<ContextEntry> = Vec::new();

    for entry in entries {
        if let Some(session_id) = &entry.session_id {
            explicit_groups
                .entry(session_id.clone())
                .or_default()
                .push(entry.clone());
        } else {
            fallback_entries.push(entry.clone());
        }
    }

    let mut groups: Vec<SessionGroup> = explicit_groups
        .into_iter()
        .map(|(session_id, entries)| SessionGroup {
            session_id,
            entries,
        })
        .collect();

    for group in &mut groups {
        group.entries.sort_by_key(|e| e.timestamp);
    }

    groups.sort_by_key(|g| g.entries.first().map_or(i64::MAX, |e| e.timestamp));

    fallback_entries.sort_by_key(|entry| entry.timestamp);

    let mut current_group: Vec<ContextEntry> = Vec::new();
    let mut current_earliest_timestamp: Option<i64> = None;
    let mut previous_timestamp: Option<i64> = None;

    for entry in fallback_entries {
        if let Some(prev_ts) = previous_timestamp {
            let gap = entry.timestamp.saturating_sub(prev_ts);
            if gap > proximity_threshold_secs {
                if let Some(earliest) = current_earliest_timestamp {
                    groups.push(SessionGroup {
                        session_id: format!("__fallback__{earliest}"),
                        entries: std::mem::take(&mut current_group),
                    });
                }
                current_earliest_timestamp = Some(entry.timestamp);
            }
        } else {
            current_earliest_timestamp = Some(entry.timestamp);
        }

        previous_timestamp = Some(entry.timestamp);
        current_group.push(entry);
    }

    if !current_group.is_empty() {
        let earliest =
            current_earliest_timestamp.expect("invariant: timestamp set when group non-empty");
        groups.push(SessionGroup {
            session_id: format!("__fallback__{earliest}"),
            entries: current_group,
        });
    }

    groups.sort_by_key(|g| g.entries.first().map_or(i64::MAX, |e| e.timestamp));

    groups
}

#[cfg(test)]
mod tests {
    use super::{group_entries_by_session, SessionGroup};
    use crate::{ContextEntry, EntryKind};

    fn make_entry(id: &str, timestamp: i64, session_id: Option<&str>) -> ContextEntry {
        ContextEntry {
            id: id.to_string(),
            content: format!("content-{id}"),
            timestamp,
            kind: EntryKind::Manual,
            token_count: None,
            session_id: session_id.map(ToString::to_string),
            compaction_count: None,
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
        }
    }

    fn assert_group_ids(groups: &[SessionGroup], expected_ids: &[&str]) {
        let actual_ids: Vec<&str> = groups
            .iter()
            .map(|group| group.session_id.as_str())
            .collect();
        assert_eq!(actual_ids, expected_ids);
    }

    #[test]
    fn empty_input_returns_empty_output() {
        let groups = group_entries_by_session(&[], 60);
        assert!(groups.is_empty());
    }

    #[test]
    fn single_entry_with_session_id_creates_one_explicit_group() {
        let entries = vec![make_entry("e1", 100, Some("s1"))];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["s1"]);
        assert_eq!(groups[0].entries.len(), 1);
        assert_eq!(groups[0].entries[0].id, "e1");
    }

    #[test]
    fn single_entry_without_session_id_creates_one_fallback_group() {
        let entries = vec![make_entry("e1", 100, None)];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["__fallback__100"]);
        assert_eq!(groups[0].entries.len(), 1);
        assert_eq!(groups[0].entries[0].id, "e1");
    }

    #[test]
    fn same_session_id_entries_are_grouped_together() {
        let entries = vec![
            make_entry("e1", 100, Some("s1")),
            make_entry("e2", 200, Some("s1")),
            make_entry("e3", 300, Some("s1")),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["s1"]);
        let ids: Vec<&str> = groups[0]
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["e1", "e2", "e3"]);
    }

    #[test]
    fn different_session_ids_create_separate_groups() {
        let entries = vec![
            make_entry("e1", 100, Some("s1")),
            make_entry("e2", 200, Some("s2")),
            make_entry("e3", 300, Some("s1")),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 2);
        assert_group_ids(&groups, &["s1", "s2"]);
        assert_eq!(groups[0].entries.len(), 2);
        assert_eq!(groups[1].entries.len(), 1);
    }

    #[test]
    fn fallback_entries_within_threshold_stay_in_one_group() {
        let entries = vec![
            make_entry("e1", 100, None),
            make_entry("e2", 150, None),
            make_entry("e3", 200, None),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["__fallback__100"]);
        assert_eq!(groups[0].entries.len(), 3);
    }

    #[test]
    fn fallback_entries_with_gap_above_threshold_split_groups() {
        let entries = vec![
            make_entry("e1", 100, None),
            make_entry("e2", 150, None),
            make_entry("e3", 300, None),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 2);
        assert_group_ids(&groups, &["__fallback__100", "__fallback__300"]);
        assert_eq!(groups[0].entries.len(), 2);
        assert_eq!(groups[1].entries.len(), 1);
    }

    #[test]
    fn mixed_explicit_and_fallback_entries_do_not_mix() {
        let entries = vec![
            make_entry("e1", 100, Some("s1")),
            make_entry("e2", 110, None),
            make_entry("e3", 120, Some("s1")),
            make_entry("e4", 130, None),
        ];

        let groups = group_entries_by_session(&entries, 30);

        assert_eq!(groups.len(), 2);
        assert_group_ids(&groups, &["s1", "__fallback__110"]);
        assert_eq!(groups[0].entries.len(), 2);
        assert_eq!(groups[1].entries.len(), 2);
    }

    #[test]
    fn unsorted_fallback_timestamps_are_sorted_before_grouping() {
        let entries = vec![
            make_entry("e3", 210, None),
            make_entry("e1", 100, None),
            make_entry("e2", 150, None),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["__fallback__100"]);

        let ordered_ids: Vec<&str> = groups[0]
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ordered_ids, vec!["e1", "e2", "e3"]);
    }

    #[test]
    fn entries_at_exact_threshold_boundary_stay_together() {
        let entries = vec![
            make_entry("e1", 100, None),
            make_entry("e2", 160, None),
            make_entry("e3", 220, None),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["__fallback__100"]);
        assert_eq!(groups[0].entries.len(), 3);
    }

    #[test]
    fn fallback_groups_interleave_chronologically_with_explicit_groups() {
        let entries = vec![
            make_entry("f1", 50, None),
            make_entry("e1", 200, Some("s1")),
            make_entry("f2", 60, None),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].session_id, "__fallback__50");
        assert_eq!(groups[1].session_id, "s1");
    }

    #[test]
    fn explicit_group_entries_are_sorted_by_timestamp_regression() {
        let entries = vec![
            make_entry("e3", 300, Some("s1")),
            make_entry("e2", 200, Some("s1")),
            make_entry("e1", 100, Some("s1")),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 1);
        assert_group_ids(&groups, &["s1"]);
        let ids: Vec<&str> = groups[0]
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["e1", "e2", "e3"]);
    }

    #[test]
    fn explicit_groups_are_sorted_by_earliest_timestamp() {
        let entries = vec![
            make_entry("a1", 200, Some("z")),
            make_entry("b1", 100, Some("a")),
        ];

        let groups = group_entries_by_session(&entries, 60);

        assert_eq!(groups.len(), 2);
        assert_group_ids(&groups, &["a", "z"]);
    }

    #[test]
    fn threshold_zero_splits_distinct_timestamps() {
        let entries = vec![
            make_entry("e1", 100, None),
            make_entry("e2", 101, None),
            make_entry("e3", 102, None),
        ];

        let groups = group_entries_by_session(&entries, 0);

        assert_eq!(groups.len(), 3);
        assert_group_ids(
            &groups,
            &["__fallback__100", "__fallback__101", "__fallback__102"],
        );
        assert_eq!(groups[0].entries.len(), 1);
        assert_eq!(groups[1].entries.len(), 1);
        assert_eq!(groups[2].entries.len(), 1);
    }

    #[test]
    fn duplicate_timestamps_with_zero_threshold_stay_in_same_group() {
        let entries = vec![
            make_entry("e1", 100, None),
            make_entry("e2", 100, None),
            make_entry("e3", 101, None),
        ];

        let groups = group_entries_by_session(&entries, 0);

        assert_eq!(groups.len(), 2);
        assert_group_ids(&groups, &["__fallback__100", "__fallback__101"]);

        let first_ids: Vec<&str> = groups[0]
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(first_ids, vec!["e1", "e2"]);
        assert_eq!(groups[1].entries.len(), 1);
    }
}
