"""Record synthetic EmailStr schema vectors; no network or mail."""
import json

from pydantic import ValidationError

from app.schemas.referral import ReferralInviteRequest

VALUES = [None, 12, '', 'not-email', 'a@localhost', 'a@', '@example.com', 'a b@example.com', '.a@example.com', 'a..b@example.com', 'a@-example.com', 'a@example.com.', 'a@exa_mple.com', 'User@EXAMPLE.COM', 'Ada <user@例子.公司>', 'a@xn--fsqu00a.xn--55qx5d', 'ü@example.com', 'x\u200b@example.com', 'a@example.test', 'a@example.invalid', 'a@127.0.0.1', 'a@b.123', 'a+b@example.com', 'a<user@example.com']
def main():
    rows = []
    for value in VALUES:
        try:
            rows.append({'input':value, 'email':ReferralInviteRequest(email=value).email})
        except ValidationError as error:
            rows.append({'input':value,'errors':error.errors(include_url=False)})
    print(json.dumps(rows, ensure_ascii=False, indent=2))
if __name__ == '__main__': main()
