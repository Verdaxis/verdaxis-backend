"""Keep the synthetic fixture trees out of pytest collection.

The pre/ and post/ trees contain deliberately-shaped Python files that stand
in for application tests. They are data for the deletion contract, never a
runnable suite: nothing here is collected, imported or executed.
"""

collect_ignore_glob = ["*"]
