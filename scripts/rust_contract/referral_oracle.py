"""Disposable referral oracle seams. No application files are changed."""
import contextvars
import json
import os


def main():
    if os.environ.get('ENVIRONMENT') != 'test':
        raise SystemExit('test only')
    from app.routers import referrals
    sequences = json.loads(os.environ.get('REFERRAL_CANDIDATE_MAP', '{}'))
    current = contextvars.ContextVar('referral_candidates')
    original_assign = referrals._assign_unique_referral_code
    original_generate = referrals.generate_referral_code

    async def assign(db, user, retries=10):
        sequence = sequences.get(str(user.id))
        if sequence is None:
            return await original_assign(db, user, retries)
        token = current.set(iter(sequence))
        try:
            return await original_assign(db, user, retries)
        finally:
            current.reset(token)

    def generate():
        sequence = current.get(None)
        return next(sequence) if sequence is not None else original_generate()

    referrals._assign_unique_referral_code = assign
    referrals.generate_referral_code = generate
    from scripts.rust_contract.phase2_oracle import main as serve
    return serve()


if __name__ == '__main__':
    main()
