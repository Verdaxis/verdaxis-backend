//! Public transport admission needs neither a database nor provider access.
use verdaxis_read::agent::llm::{ErrorCode, models};

#[test]
fn phase_a_model_allow_list_is_closed() {
    assert_eq!(models::MODELS.len(), 1);
    assert!(models::settings("deepseek-v4.1-flash").is_ok());
    assert!(matches!(
        models::settings("unlisted"),
        Err(ErrorCode::ModelNotAllowed)
    ));
}
