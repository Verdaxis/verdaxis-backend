"""Record synthetic EmailStr schema vectors from the pinned Python oracle.

The output is the Python outcome for every input: the normalized address, or
pydantic's exact error list. Inputs are synthetic and generated across the
address grammar: ordinary and special-character addresses, internationalized
local parts and domains, quoted local parts, display names, IP literals,
special-use names, whitespace, normalization and every length boundary. No
network, mail or database is used.

    venv/bin/python -m scripts.rust_contract.referral_email_vectors > tests/rust_contract/phase2/referral-email-vectors.json
"""
import json
import sys
import unicodedata

from app.schemas.referral import ReferralInviteRequest
from pydantic import ValidationError

MAILBOX_NAMES = ['info', 'marketing', 'sales', 'support', 'abuse', 'noc', 'security', 'postmaster',
                 'hostmaster', 'usenet', 'news', 'webmaster', 'www', 'uucp', 'ftp']
ASCII_SPECIALS = "!#$%&'*+-/=?^_`{|}~"
# Characters that fall in each Unicode general-category family the checks treat differently.
SAMPLE_CHARACTERS = [
    'é', 'ß', 'İ', 'ǅ', 'Σ', 'ς', 'Ж', 'א', 'ا', 'ก', '฿',
    'ᄀ', 'あ', 'ア', '中', '한', 'Ａ', 'ａ', '０', '٠', '۰', '①',
    '½', 'Ⅰ', '€', '©', '☃', '❤', '\U0001f600', '\U0001f1fa', '́', '̸',
    'ा', '⃝', '\u200b', '‌', '‍', '‎', '‏', '‪', '‮', '⁦', '⁩',
    '­', '⁠', '﻿', '؜', ' ', ' ', ' ', '　', ' ', ' ', '\u0085',
    '', '\U000f0000', '￿', '￾', '͸', '\U0001fffe', '\U000e0001', '\U000e0041', '․',
    '‥', '…', '。', '．', '｡', '⒈', '⑴', '﹫', '﹓', '`', 'ʼ',
    '·', '͵', '׳', '・', 'ـ', '᠋',
]

# Code points that once fell between generated table runs (compatibility ideographs,
# private use and their neighbours); kept as integers so no formatter rewrites them.
BOUNDARY_CODE_POINTS = [0xFA14, 0xFA1F, 0xF900, 0xFAD9, 0xFA6E, 0xF8FF, 0xE001, 0xD7FF, 0xE000, 0xFFFD]
SAMPLE_CHARACTERS += [chr(code) for code in BOUNDARY_CODE_POINTS]


def length_vectors():
    """Boundaries for the local part, labels, domain, address and pydantic limit."""
    label63, label64 = 'b' * 63, 'b' * 64
    domain253 = '.'.join(['a' * 63, 'a' * 63, 'a' * 63, 'a' * 61])
    domain189 = f"{'b' * 63}.{'b' * 63}.{'b' * 57}.com"
    values = [
        'a' * 63 + '@example.com', 'a' * 64 + '@example.com', 'a' * 65 + '@example.com',
        f'x@{label63}.com', f'x@{label64}.com', f'x@{label63}', f'x@{label63}.{label63}',
        f'x@{domain253}', f'x@{domain253}a', f'x@{domain253}.',
        'a' * 64 + '@' + domain189, 'a' * 64 + '@' + domain189 + 'c', 'a' * 63 + '@' + domain189,
        'a' * 65 + '@' + domain189[:-1], 'a' * 100 + '@' + domain189, 'a' * 190 + '@example.com',
        'a' * 242 + '@example.com', 'a' * 243 + '@example.com', 'a' * 244 + '@example.com',
        'a' * 2035 + '@example.com', 'a' * 2036 + '@example.com', 'a' * 2037 + '@example.com',
        'a' * 3000 + '@example.com', 'x@' + 'a.' * 126 + 'a', 'x@' + 'a.' * 126 + 'ab', 'x@' + 'a.' * 130 + 'com',
        'é' * 60 + '@example.com', 'é' * 121 + '@example.com', 'é' * 122 + '@example.com',
        'é' * 123 + '@example.com', '中' * 80 + '@example.com', '中' * 81 + '@example.com',
        '\U0001f600' * 60 + '@example.com', 'a@' + '中' * 15 + '.com', 'a@' + '中' * 16 + '.com',
        'a@' + '中' * 40 + '.com', 'a@' + 'é' * 30 + '.com', 'a@' + 'é' * 45 + '.com',
        'a@' + '.'.join(['中' * 14] * 4) + '.com', 'a@' + '.'.join(['中' * 12] * 5) + '.com',
        'a@' + '.'.join(['b' * 50] * 5) + '.com', ' ' * 10 + 'a@example.com' + ' ' * 10,
        ' ' * 1100 + 'a@example.com', 'a@example.com' + ' ' * 2040, 'Ada <' + 'a' * 250 + '@example.com>',
        'A' * 2040 + ' <a@example.com>', '"' + 'q' * 70 + '"@example.com', '"' + 'q' * 300 + '"@example.com',
    ]
    return values


def domain_vectors():
    labels = ['münchen', 'βόλος', 'ΒΌΛΟΣ', 'faß', 'FASS', 'ǅ',
              'İstanbul', 'ⓐ', '㎡', 'ＡＢＣ', '例え', '日本語', 'пример',
              'مثال', 'דוגמה', '한국', 'ไทย', 'ශ්‍රී',
              'क्‍ष', 'a­b', 'a\u200bb', 'a‌b', 'a‍b', '́a', 'á', '☃', '\U0001f600',
              'café', 'café', '·', 'l·l', 'a͵', '׳a', 'a・', '・ア', '٠۰', 'ا٠',
              'a。b', 'a．b', 'a｡b', '⒈', '⑴', '`', '﹫', 'ʼ', 'ـ', '᠋', '․', 'xn--',
              'xn--a', 'xn--zz', 'xn--ab-', 'XN--55QW42G', 'xn--55qw42g', 'xn--fsqu00a', 'xn--0', 'xn--a--', 'xn---', 'xn--9ca',
              'xn--80ak6aa92e', 'xn--mnchen-3ya', 'xn--n3h', 'ab--c', 'ab--', '--ab', 'a-', '-a', 'a_b', 'a b', 'a%20b', 'a/b', 'a@b',
              'a:b', 'a+b', 'exämple', 'EXAMPLE', 'ExAmPlE']
    values = []
    for label in labels:
        values += [f'a@{label}.com', f'a@example.{label}', f'a@{label}']
    punycode = []
    for label in ['例え', 'münchen', 'пример', 'דוגמה', 'a‍b', '١a', 'a١',
                  'אa', 'aא', 'א1', 'ا١۱', '́a', 'a·a', 'l·l', '☃', '\U0001f600', '͸',
                  'İ', 'A', 'a̸', 'ก́', '가', '・', 'ア・', '͵α', 'α͵']:
        try:
            punycode.append('xn--' + label.encode('punycode').decode('ascii'))
        except UnicodeError:
            pass
    for label in punycode:
        values += [f'a@{label}.com', f'a@{label.upper()}.com', f'a@x.{label}.org']
    values += [
        'a@example.com.', 'a@.example.com', 'a@example..com', 'a@exam..ple.com', 'a@-example.com', 'a@example-.com',
        'a@exa-.mple.com', 'a@exa.-mple.com', 'a@example.c', 'a@example.co-', 'a@example.c0m', 'a@example.123', 'a@example.1a', 'a@1.2.3.4',
        'a@a.b.c.d.e.f.g.h.i.j.k.l', 'a@e.x', 'a@localhost', 'a@a.localhost', 'a@a.test', 'a@test', 'a@a.example', 'a@example.invalid', 'a@invalid',
        'a@a.local', 'a@local', 'a@a.onion', 'a@onion', 'a@a.arpa', 'a@in-addr.arpa', 'a@arpa', 'a@a.b.test', 'a@Test.COM', 'a@a.TEST', 'a@test.invalid.com',
        'a@a..b', 'a@a.b..', 'a@.', 'a@..', 'a@@example.com', 'a@b@example.com', 'a@example.com@', 'a@ example.com', 'a@example.com ', 'a@exa mple.com',
        'a@exa\tmple.com', 'a@exa\nmple.com', 'a@exa\x00mple.com', 'a@exa\x7fmple.com', 'a@[127.0.0.1]', 'a@[1.2.3]', 'a@[1.2.3.4.5]', 'a@[256.1.1.1]',
        'a@[01.1.1.1]', 'a@[IPv6:::1]', 'a@[IPv6:2001:db8::1]', 'a@[IPv6:2001:db8:0:0:0:0:0:1]', 'a@[IPv6:zz]', 'a@[IPv6:]', 'a@[IPV6:::1]', 'a@[ipv6:::1]',
        'a@[tag:value]', 'a@[tag]', 'a@[a b]', 'a@[]', 'a@[', 'a@]', 'a@[x]', 'a@[1.2.3.4', 'a@1.2.3.4]', 'a@[[1.2.3.4]]', 'a@[::1]', 'a@[IPv6:1.2.3.4]',
        'a@[IPv6:::ffff:1.2.3.4]', 'a@[é]', 'a@[Tag:é]', 'a@[1.2.3.٤]', 'a@[:]', 'a@[x:y]',
    ]
    return values


def local_vectors():
    values = ['a@example.com', 'A@EXAMPLE.COM', 'User.Name@Example.Com', 'first.last+tag@example.co.uk', 'a.b.c@example.com',
              '1@example.com', '_@example.com', '-@example.com', '+@example.com', '9a@example.com', 'a..b@example.com', '.a@example.com',
              'a.@example.com', '.@example.com', '..@example.com', '@example.com', '@', 'a', 'a@', '', ' ', '  a@example.com  ', 'a @example.com',
              ' a@example.com', 'a@example.com\n', '\na@example.com', 'a@example.com\r\n', '\ta@example.com\t', 'a\u0000@example.com']
    for character in ASCII_SPECIALS:
        values += [f'{character}@example.com', f'a{character}b@example.com', f'a{character}@example.com', f'{character}a@example.com']
    for code in list(range(0x20)) + [0x7f]:
        values += [f'a{chr(code)}b@example.com', f'{chr(code)}a@example.com', f'a@exam{chr(code)}ple.com']
    for character in '"(),.:;<>@[\\] ':
        values += [f'a{character}b@example.com', f'{character}a@example.com', f'a{character}@example.com']
    for name in MAILBOX_NAMES:
        values += [f'{name}@example.com', f'{name.upper()}@example.com', f'{name.title()}@Example.com', f'{name}.x@example.com']
    values += ['INFO @example.com', 'Postmaster@example.com', 'Admin@example.com', 'ADMIN@EXAMPLE.COM', 'İNFO@example.com', 'infö@example.com']
    for character in SAMPLE_CHARACTERS:
        category = unicodedata.category(character)
        values += [f'a{character}b@example.com', f'{character}a@example.com', f'a{character}@example.com', f'a@exam{character}ple.com',
                   f'a@{character}example.com', f'a@example{character}.com', f'a@example.c{character}m']
    values += ['é@example.com', 'é@example.com', 'éé@example.com', 'Å@example.com', 'Å@example.com',
               'Ω@example.com', '가@example.com', '각@example.com', 'ａｂ@example.com',
               'ﬁ@example.com', 'µ@example.com', 'Ⅰ@example.com', 'á̸@example.com', '́@example.com',
               'a@́example.com', '@́a@example.com', '＠@example.com', 'a＠example.com', 'a﹫' + 'example.com',
               '用户@例子.广告', 'ユーザー@例え.jp', 'пользователь@пример.рф',
               'üser@example.com', 'Ada\u200b@example.com', 'Ada‮@example.com', 'a b@example.com', 'ab͸@example.com']
    return values


def quoted_and_display_vectors():
    return [
        '"a"@example.com', '"a b"@example.com', '"a@b"@example.com', '"a\\"b"@example.com', '"a\\\\b"@example.com', '""@example.com',
        '"a"b@example.com', '"a@example.com', 'a"@example.com', 'a"b"@example.com', '"a".b@example.com', '"a"."b"@example.com',
        '"a\tb"@example.com', '"a\nb"@example.com', '"a\x00b"@example.com', '"a\x7fb"@example.com', '"é"@example.com',
        '"中文"@example.com', '"a\u200bb"@example.com', '"á"@example.com', '"́a"@example.com', '"a b"@例.com',
        '" "@example.com', '"."@example.com', '".a"@example.com', '"a."@example.com', '"a..b"@example.com', '"a(b)"@example.com',
        '"a<b>"@example.com', '"a\\a"@example.com', '"\\"@example.com', '"\\\\"@example.com', '"a"@', '"a"@[127.0.0.1]', '"a"@x@example.com',
        '"INFO"@example.com', '"info"@example.com', '"a\\ b"@example.com', '"a\\é"@example.com', '"' + 'a' * 70 + '"@example.com',
        'Ada <a@example.com>', 'Ada Lovelace <a@example.com>', '<a@example.com>', ' <a@example.com>', 'Ada<a@example.com>',
        'Ada  <a@example.com>', 'Ada <a@example.com> ', ' Ada <a@example.com> ', '\tAda\t<a@example.com>\t', 'Ada <a@example.com>\n',
        '\nAda <a@example.com>', 'Ada\n<a@example.com>', 'Ada <a@example.com\n>', 'Ada <\na@example.com>', 'Ada <a@example.com>>',
        'Ada <<a@example.com>', 'Ada <a@example.com> x', 'x Ada <a@example.com>', 'Ada <a@example.com', 'Ada a@example.com>', 'Ada < a@example.com>',
        'Ada <a@example.com >', 'Ada <>', 'Ada < >', '<>', '<', '>', '<<>>', 'Ada <a>', 'Ada <@example.com>', 'Ada <a@>', 'Ada <a@example>',
        '"Ada Lovelace" <a@example.com>', '"Ada" <a@example.com>', '"" <a@example.com>', '"A\\"B" <a@example.com>', '"A <b>" <a@example.com>',
        '"A" "B" <a@example.com>', '"A"B <a@example.com>', 'A "B" <a@example.com>', '"Ada <a@example.com>', 'Ada" <a@example.com>',
        'A.B <a@example.com>', 'A,B <a@example.com>', 'A@B <a@example.com>', 'A(B) <a@example.com>', 'A:B <a@example.com>',
        "O'Brien <a@example.com>", 'A!#$%&*+-/=?^_`{|}~ <a@example.com>', 'Aé <a@example.com>', 'Áda <a@example.com>',
        '中文 <a@example.com>', 'A\u200bB <a@example.com>', 'A B <a@example.com>', 'A　B <a@example.com>',
        'A\x1cB <a@example.com>', '\x1c<a@example.com>', '<a@example.com>\x1f', '\u0085<a@example.com>', '<a@example.com> ',
        'A_B <a@example.com>', 'A1 <a@example.com>', '1 <a@example.com>', 'A  B  C <a@example.com>', 'A ́ <a@example.com>',
        'Á <a@example.com>', '́A <a@example.com>', '"Á" <a@example.com>', 'A ≮a@example.com>', 'A≮ <a@example.com>',
        'Ada <a@example.com> <b@example.com>', 'Ada <a@example.com><b@example.com>', 'Ada <a@example.com>, Bob <b@example.com>',
        '<Ada <a@example.com>>', 'Ada <"a"@example.com>', 'Ada <"a b"@example.com>', 'Ada <a b@example.com>', 'Ada <a@exa mple.com>',
        'Ada <INFO@Example.COM>', 'Ada <a@例.com>', 'Ada <é@example.com>', '"Ada" <"a"@example.com>', 'Ada <a@[127.0.0.1]>',
        'Ada <a@example.com>' + ' ' * 5, '<a@example.com> <', 'a@example.com <b@example.com>', 'a <b@example.com> c@example.com',
        'a@example.com>', '<a@example.com', 'a<b@example.com', 'a>b@example.com', 'a@exam<ple.com', 'a@exam>ple.com', '<a@exam>ple.com>',
        '"<a@example.com>"', '"a@example.com"', 'mailto:a@example.com', 'a@example.com;', 'a@example.com,', 'a@example.com, b@example.com',
        'a@example.com b@example.com', "'a'@example.com", '(c)a@example.com', 'a(c)@example.com', 'a@(c)example.com', 'a@example(c).com',
        '\\a@example.com', 'a\\@example.com', 'a@example.com\\', '＠', 'a＠example.com', 'a﹫example.com', 'a﹫',
    ]


def vectors():
    values = [None, 12, True, 1.5, [], {}, ['a@example.com'], {'email': 'a@example.com'}]
    for group in (local_vectors(), domain_vectors(), quoted_and_display_vectors(), length_vectors()):
        values += group
    seen = set()
    unique = []
    for value in values:
        key = json.dumps(value, ensure_ascii=True)
        if key not in seen:
            seen.add(key)
            unique.append(value)
    return unique


def has_surrogate(value):
    return isinstance(value, str) and any(0xD800 <= ord(character) < 0xE000 for character in value)


def record(value):
    try:
        return {'input': value, 'email': ReferralInviteRequest(email=value).email}
    except ValidationError as error:
        return {'input': value, 'errors': error.errors(include_url=False)}


def main():
    rows = [record(value) for value in vectors() if not has_surrogate(value)]
    print('[')
    print(',\n'.join('  ' + json.dumps(row, ensure_ascii=False, separators=(', ', ': ')) for row in rows))
    print(']')
    print(f'{len(rows)} vectors', file=sys.stderr)


if __name__ == '__main__':
    main()
