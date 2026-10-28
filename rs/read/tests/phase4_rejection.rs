use verdaxis_read::business::admission::revocation_failure;

#[test]
fn p4_t2_c07() {
    let missing = revocation_failure(sqlx::Error::RowNotFound);
    match missing {
        verdaxis_read::business::transaction::AttemptError::Application(error) => {
            assert_eq!(error.status.as_u16(), 404);
            assert_eq!(error.detail["code"], "USER_NOT_FOUND");
        }
        other => panic!("unexpected refusal: {other:?}"),
    }
}
