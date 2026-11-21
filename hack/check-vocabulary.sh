#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - "$@" <<'EOF'
import hashlib
import os
import re
import subprocess
import sys

DIGESTS = "hack/vocabulary.sha256"
COMPACT = "hack/vocabulary-compact.sha256"
FLAGS = {"--self-test", "--diffs"}
SKIPPED = {"hack/vocabulary.sha256", "frontend/pnpm-lock.yaml", "Cargo.lock"}
MAX_BYTES = 3 * 1024 * 1024
TOKEN = re.compile(r"[a-z0-9]+")


def digest_of(term):
    return hashlib.sha256(term.encode("utf-8")).hexdigest()


def candidates(tokens):
    terms = set(tokens)
    for size in (2, 3):
        for start in range(len(tokens) - size + 1):
            terms.add("".join(tokens[start : start + size]))
    return terms


def matches(line, main, compact):
    tokens = TOKEN.findall(line.lower())
    found = set()
    for term in candidates(tokens):
        label = main.get(digest_of(term))
        if label is not None:
            found.add(label)
    for token in tokens:
        label = compact.get(digest_of(token))
        if label is not None:
            found.add(label)
    return found


def scan(text, main, compact):
    hits = set()
    for lineno, line in enumerate(text.split("\n"), 1):
        for label in matches(line, main, compact):
            hits.add((lineno, label))
    return sorted(hits)


def load_digests(path, label):
    digests = {}
    with open(path, encoding="utf-8") as handle:
        for number, line in enumerate(handle, 1):
            value = line.strip()
            if value:
                digests.setdefault(value, f"{label} #{number}")
    return digests


def git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout


def diff_place(sha, path, main, compact, position):
    if not path:
        return f"commit {sha}"
    if not matches(path, main, compact):
        return f"commit {sha}, {path}"
    if path in position:
        return f"commit {sha}, tracked file #{position[path]}"
    return f"commit {sha}"


def self_test():
    main = {digest_of("probe"): "digest #1", digest_of("twowords"): "digest #2"}
    triple = {**main, digest_of("abc"): "digest #3"}
    compact = {digest_of("onetoken"): "digest compact #1"}
    cases = [
        ("A Probe here", main, {}, True),
        ("two words", main, {}, True),
        ("two\nwords", main, {}, False),
        ("unrelated text", main, {}, False),
        ("a b c", triple, {}, True),
        ("onetoken", {}, compact, True),
        ("one token", main, compact, False),
    ]
    failed = False
    for text, table, singles, expected in cases:
        if bool(scan(text, table, singles)) != expected:
            print(f"self-test failed on {text!r}: expected a hit = {expected}", file=sys.stderr)
            failed = True
    if failed:
        sys.exit(1)
    print("self-test passed")


def main(arguments):
    if not arguments <= FLAGS:
        print("usage: hack/check-vocabulary.sh [--self-test] [--diffs]", file=sys.stderr)
        sys.exit(2)
    if "--self-test" in arguments:
        self_test()
        return

    main_digests = load_digests(DIGESTS, "digest")
    compact_digests = load_digests(COMPACT, "digest compact")
    problems = []

    paths = [os.fsdecode(raw) for raw in git("ls-files", "-z").split(b"\0") if raw]
    position = {name: number for number, name in enumerate(paths, 1)}

    hit_paths = set()
    for name in paths:
        labels = matches(name, main_digests, compact_digests)
        if labels:
            hit_paths.add(name)
        for label in sorted(labels):
            problems.append(f"file name of tracked file #{position[name]}: a forbidden term ({label})")

    for name in paths:
        if name in SKIPPED or not os.path.isfile(name) or os.path.islink(name):
            continue
        if os.path.getsize(name) > MAX_BYTES:
            continue
        with open(name, "rb") as handle:
            data = handle.read()
        if b"\0" in data:
            continue
        where = f"tracked file #{position[name]}" if name in hit_paths else name
        for lineno, label in scan(data.decode("utf-8", errors="ignore"), main_digests, compact_digests):
            problems.append(f"{where}:{lineno}: a forbidden term ({label})")

    records = [raw for raw in git("log", "--all", "-z", "--format=%h%x1f%B").split(b"\0") if raw.strip()]
    for raw in records:
        sha, _, body = raw.decode("utf-8", errors="ignore").partition("\x1f")
        sha = sha.strip()
        for label in sorted({label for _, label in scan(body, main_digests, compact_digests)}):
            problems.append(f"commit {sha}: a forbidden term ({label})")

    if "--diffs" in arguments:
        output = git("log", "--all", "-p", "--format=%x1e%h").decode("utf-8", errors="ignore")
        for record in output.split("\x1e")[1:]:
            sha, _, body = record.partition("\n")
            sha = sha.strip()
            path = ""
            in_header = False
            found = set()
            for line in body.split("\n"):
                if line.startswith("diff --git "):
                    in_header, path = True, ""
                elif in_header and line.startswith("@@"):
                    in_header = False
                elif in_header and (line.startswith("+++ b/") or line == "+++ /dev/null"):
                    path = line[6:] if line.startswith("+++ b/") else ""
                elif line.startswith("+"):
                    found.update((path, label) for label in matches(line, main_digests, compact_digests))
            for path, label in sorted(found):
                place = diff_place(sha, path, main_digests, compact_digests, position)
                problems.append(f"{place}: a forbidden term ({label})")

    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        sys.exit(1)
    print(f"vocabulary checked: {len(paths)} files, {len(records)} commit messages")


main(set(sys.argv[1:]))
EOF
