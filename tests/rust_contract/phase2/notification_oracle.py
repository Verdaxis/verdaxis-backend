"""Print notification DTO and validation vectors from the unchanged Python source."""
import json
from uuid import UUID

from pydantic import TypeAdapter, ValidationError

from app.routers.notifications import NotificationResponse


def main():
    result = {"uuid_errors": {}, "invalid_data": []}
    for value in ("not-a-uuid", "", "1234", "{20000000-0000-4000-8000-00000000000a}", "urn:uuid:20000000-0000-4000-8000-00000000000a"):
        try:
            parsed = TypeAdapter(UUID).validate_python(value)
            result["uuid_errors"][value] = {"value": str(parsed)}
        except ValidationError as error:
            result["uuid_errors"][value] = {"errors": error.errors(include_url=False)}
    for value in ("scalar", [], 3, True):
        try:
            NotificationResponse.model_validate({
                "id": "30000000-0000-4000-8000-000000000001", "type": "SYSTEM",
                "title": "Title", "message": "Message", "data": value,
                "is_read": False, "created_at": "2026-10-18T00:00:00Z",
            })
        except ValidationError:
            result["invalid_data"].append(value)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
