use demo::textstats::word_freq;

fn pairs(v: &[(&str, usize)]) -> Vec<(String, usize)> {
    v.iter().map(|(w, n)| (w.to_string(), *n)).collect()
}

#[test]
fn hidden() {
    assert_eq!(word_freq("The cat and the hat. The END, and", 2), pairs(&[("the", 3), ("and", 2)]));
    assert_eq!(word_freq("don't Don't stop", 5), pairs(&[("don't", 2), ("stop", 1)]));
    assert_eq!(word_freq("b a c a b", 3), pairs(&[("a", 2), ("b", 2), ("c", 1)]));
    assert_eq!(word_freq("café cafe", 5), pairs(&[("caf", 1), ("cafe", 1)]));
    assert_eq!(word_freq("x2y", 5), pairs(&[("x", 1), ("y", 1)]));
    assert!(word_freq("", 3).is_empty());
}
