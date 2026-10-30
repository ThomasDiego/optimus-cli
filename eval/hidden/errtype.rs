use demo::age::{parse_age, AgeError};
use demo::profile::parse_profile;

#[test]
fn hidden() {
    assert_eq!(parse_age(" 42 "), Ok(42));
    assert_eq!(parse_age(""), Err(AgeError::Empty));
    assert_eq!(parse_age("abc"), Err(AgeError::NotNumber));
    assert_eq!(parse_age("200"), Err(AgeError::TooOld));
    assert_eq!(parse_profile("bob,7").unwrap().age, 7);
    assert!(parse_profile("bob,x").is_err());
    assert!(parse_profile("bob").is_err());
}
