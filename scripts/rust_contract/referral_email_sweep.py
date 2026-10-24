"""Write a Unicode sweep of EmailStr outcomes from the pinned Python oracle.

Every code point is placed in several address positions; the Rust side replays
the file (`emailstr_unicode_sweep_matches_python`, an ignored test) and reports
every difference. Letters outside the BMP-heavy bulk blocks, unassigned and
private-use code points are sampled, because all members of those groups
follow the same rule.

    venv/bin/python -m scripts.rust_contract.referral_email_sweep > work/rust-phase2/email-sweep.tsv

Each line is `position<TAB>status<TAB>json(input)<TAB>json(outcome)`, with
status `O` (normalized address) or `E` (pydantic reason text).
"""
import json
import sys
import unicodedata

from app.schemas.referral import ReferralInviteRequest
from pydantic import ValidationError

POSITIONS = {
    'local-middle': lambda c: f'a{c}b@example.com',
    'local-first': lambda c: f'{c}a@example.com',
    'domain-middle': lambda c: f'a@ex{c}ample.com',
    'domain-first': lambda c: f'a@{c}xample.com',
    'domain-last': lambda c: f'a@example{c}.com',
    'quoted-local': lambda c: f'"a{c}b"@example.com',
    'display-name': lambda c: f'a{c}b <a@example.com>',
    'quoted-display-name': lambda c: f'"a{c}b" <a@example.com>',
    'domain-literal': lambda c: f'a@[{c}1.2.3.4]',
}
SAMPLE_EVERY = {'Lo': 11, 'Cn': 53, 'Co': 97, 'Lm': 3, 'Mn': 1, 'So': 2}


def selected(code):
    category = unicodedata.category(chr(code))
    step = SAMPLE_EVERY.get(category, 1)
    return code % step == 0


def outcome(value):
    try:
        return 'O', ReferralInviteRequest(email=value).email
    except ValidationError as error:
        return 'E', error.errors(include_url=False)[0]['ctx']['reason']


def main():
    count = 0
    for code in range(0x110000):
        if 0xD800 <= code < 0xE000 or not selected(code):
            continue
        character = chr(code)
        for name, build in POSITIONS.items():
            value = build(character)
            status, text = outcome(value)
            sys.stdout.write(f'{name}\t{status}\t{json.dumps(value)}\t{json.dumps(text)}\n')
            count += 1
    print(f'{count} sweep lines', file=sys.stderr)


if __name__ == '__main__':
    main()
