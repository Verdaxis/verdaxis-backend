"""Deterministic Python Decimal operation vectors for Task 11's arithmetic gate."""
import json
import random
from decimal import ROUND_HALF_UP, Decimal, InvalidOperation, localcontext
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def vectors():
    random_source = random.Random(110028)
    results = []
    with localcontext() as context:
        context.prec = 28
        pairs = []
        for _ in range(250):
            left = Decimal(random_source.randrange(-10**24, 10**24)).scaleb(-random_source.randrange(0, 24))
            right = Decimal(random_source.randrange(1, 10**24)).scaleb(-random_source.randrange(0, 24))
            pairs.append((left, right))
        pairs.extend((Decimal(left), Decimal(right)) for left, right in [
            ("0", "3"), ("-0.00", "3"), ("1", "3"), ("2.675", "1"),
            ("-2.675", "1"), ("1.000", "2.0"), ("1E-29", "3"),
            ("9999999999999999999999999.995", "1"),
            ("0E-100", "2"), ("-0.00", "-0.0"),
        ])
        for left, right in pairs:
            operations = {
                "add": lambda: left + right,
                "subtract": lambda: left - right,
                "multiply": lambda: left * right,
                "divide": lambda: left / right,
            }
            for operation, calculate in operations.items():
                if operation == "divide" and right == 0:
                    continue
                result = calculate()
                try:
                    money = str(result.quantize(Decimal(".01"), rounding=ROUND_HALF_UP))
                except InvalidOperation:
                    continue
                results.append({"left": str(left), "right": str(right), "operation": operation,
                                "result": str(result), "money": money})
    return results


if __name__ == "__main__":
    (ROOT / "deploy/rust/voyage-decimal-vectors.json").write_text(json.dumps(vectors(), indent=2) + "\n")
