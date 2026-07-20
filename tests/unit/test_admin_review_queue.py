from app.routers import auth_simple


def test_admin_review_queue_and_detail_routes_are_exposed():
    paths = {route.path for route in auth_simple.router.routes}
    assert "/auth/admin/review-queue" in paths
    assert "/auth/admin/review-queue/{user_id}" in paths


def test_review_projection_contains_operator_decision_metadata():
    required = {
        "email",
        "email_verified",
        "account_status",
        "kyc_status",
        "kyc_organization_id",
        "kyc_external_evidence_reference",
        "kyc_review_note",
        "current_organization",
        "requested_organizations",
    }
    assert required <= auth_simple.ADMIN_REVIEW_PROJECTION_FIELDS
