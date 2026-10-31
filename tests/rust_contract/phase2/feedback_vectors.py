"""Generate synthetic feedback validation vectors from the pinned Python schema.

Run only in the disposable test environment. No database or live data is used.
"""
import json
from pathlib import Path

from fastapi.encoders import jsonable_encoder
from pydantic import ValidationError

from app.routers.feedback import FeedbackSubmission


def main():
    root = Path(__file__).resolve().parent
    submissions = [
        ("empty", {"message": ""}),
        ("one", {"message": "x"}),
        ("max", {"message": "x" * 2000}),
        ("over-max", {"message": "x" * 2001}),
        ("blank", {"message": " \t\n"}),
        ("unicode-blank", {"message": "\u001c\u0085 　"}),
        ("strip", {"message": "\u001c  hello 　"}),
        ("unicode-max", {"message": "🚢" * 2000}),
        ("missing", {}),
        ("wrong-type", {"message": 123, "page": False}),
        ("null", {"message": None}),
        ("not-object", []),
    ]
    for name, page in [
        ("path", "/app/marketplace"), ("url", "https://example.invalid/app"),
        ("query", "/app?x=1"), ("fragment", "/app#x"),
        ("slashes", "//app"), ("null", None),
        ("max", "/" + "x" * 199), ("over-max", "/" + "x" * 200),
    ]:
        submissions.append((f"page-{name}", {"message": "kept", "page": page}))
    cases = []
    for name, body in submissions:
        row = {"id": name, "body": body}
        try:
            value = FeedbackSubmission.model_validate(body, from_attributes=True)
            row.update(result="accepted", **value.model_dump())
        except ValidationError as error:
            errors = jsonable_encoder(error.errors(include_url=False))
            for entry in errors:
                entry["loc"] = ["body", *entry["loc"]]
            row.update(result="rejected", errors=errors)
        cases.append(row)
    manifest = json.loads((root / "evidence-manifest.json").read_text())
    required = [row["id"] for row in manifest["required_scenarios"]
                if row["route_id"] in ("F01", "F02")]
    result = {
        "schema_version": 1,
        "group": "feedback",
        "route_ids": ["F01", "F02"],
        "runner_status": "pending the runner lane",
        "submission_cases": cases,
        "query_cases": [
            {"id": "defaults", "query": "", "result": "accepted", "limit": 50, "offset": 0},
            {"id": "bounds", "query": "limit=200&offset=99", "result": "accepted", "limit": 200, "offset": 99},
            {"id": "repeated", "query": "limit=1&limit=2", "result": "accepted", "limit": 2, "offset": 0},
        ],
        "limits": {"feedback": {"per_minute": 5}, "admin_feedback": {"per_minute": 60}, "admin_prefix_per_minute": 300},
        "required_cases": required,
        "scenarios": [
            {"id": "feedback-submit-repeat", "route_id": "F01", "actor": "approved-buyer", "actions": [
                {"method": "POST", "path": "/api/feedback", "headers": {"Idempotency-Key": "ignored"}, "body": {"message": "  voluntary  ", "page": "/app"}, "status": 201},
                {"method": "POST", "path": "/api/feedback", "headers": {"Idempotency-Key": "ignored"}, "body": {"message": "  voluntary  ", "page": "/app"}, "status": 201}],
             "committed_state": {"feedback_entries_delta": 2, "message": "voluntary", "page": "/app", "other_business_tables": "unchanged", "email_attempts": 0}},
            {"id": "feedback-admin-list", "route_id": "F02", "actor": "approved-admin", "fixtures": ["identified-author", "author-without-organization", "empty-name", "created-time-ties"], "actions": [
                {"method": "GET", "path": "/api/admin/feedback?limit=200", "status": 200},
                {"method": "GET", "path": "/api/admin/feedback?offset=999", "status": 200}], "committed_state": "unchanged"},
            {"id": "feedback-admin-denied", "route_id": "F02", "actor": "approved-buyer", "actions": [{"method": "GET", "path": "/api/admin/feedback", "status": 403}], "committed_state": "unchanged"},
        ],
    }
    (root / "feedback.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
