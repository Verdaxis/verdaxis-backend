use verdaxis_read::routes::orderbook_lists::page_offset;

#[test]
fn page_aliases_override_skip_and_limit() {
    assert_eq!(page_offset(99, 50, Some(2), Some(3)), Some((3, 3)));
    assert_eq!(page_offset(7, 50, None, Some(3)), Some((7, 3)));
    assert_eq!(page_offset(7, 50, Some(1), None), Some((0, 50)));
}

#[test]
fn page_offset_rejects_zero_page_and_overflow() {
    assert_eq!(page_offset(0, 50, Some(0), None), None);
    assert_eq!(page_offset(0, 100, Some(u64::MAX), None), None);
}
