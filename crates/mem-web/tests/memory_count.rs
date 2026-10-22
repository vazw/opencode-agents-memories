//! The upstream `web/src/lib/memory-count.ts` client contract.
//!
//! `GET /api/memories` and `GET /api/search` answer two different totals; the
//! explorer shows the search total while a search or a tag filter is active
//! and the store total otherwise.

use mem_web::api::client::displayed_memory_count;

#[test]
fn test_displayed_memory_count_shows_matched_results_while_search_is_active() {
    assert_eq!(displayed_memory_count(true, 3, 120, false), 3);
    assert_eq!(displayed_memory_count(true, 0, 120, false), 0);
}

#[test]
fn test_displayed_memory_count_restores_the_store_total_when_search_is_cleared() {
    assert_eq!(displayed_memory_count(false, 3, 120, false), 120);
}

#[test]
fn test_displayed_memory_count_shows_the_filtered_total_when_a_tag_filter_is_active() {
    assert_eq!(displayed_memory_count(false, 3, 120, true), 3);
    assert_eq!(displayed_memory_count(false, 0, 120, true), 0);
}

#[test]
fn test_displayed_memory_count_prefers_the_search_total_when_both_search_and_tag_filter_are_active()
{
    assert_eq!(displayed_memory_count(true, 2, 120, true), 2);
}

#[test]
fn test_displayed_memory_count_keeps_the_store_total_when_no_search_and_no_tag_filter() {
    assert_eq!(displayed_memory_count(false, 3, 120, false), 120);
}
