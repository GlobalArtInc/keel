#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'EOF'
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(line_buffering=True)
problems = []

M1_NAMES = {
    "Authenticator": "Authenticator",
    "Client": "Client",
    "Authenticated": "Authenticated",
    "SessionChange": "SessionChange",
    "SecretCipher": "SecretCipher",
    "SecondFactorSource": "SecondFactorSource",
    "parent_id_hash": "parent_id_hash",
    "su_id_hash": "su_id_hash",
    "SettingDef": "SettingDef",
    "SettingsSource": "SettingsSource",
    "PreferenceSource": "PreferenceSource",
    "Call::tz": "fn tz(",
    "ConfigSection": "ConfigSection",
    "Flush": "Flush",
}


def found(pattern, root):
    result = subprocess.run(
        ["grep", "-rqF", "--", pattern, root],
        check=False,
    )
    return result.returncode == 0


for name, code in M1_NAMES.items():
    if not found(name, "docs/design"):
        problems.append(f"the M1 name {name} is not found under docs/design/")
    if not found(code, "crates"):
        problems.append(f"the M1 name {name} is not found under crates/ as {code!r}")
print(f"{len(M1_NAMES)} M1 names looked up under docs/design/ and crates/")


def section(text, heading):
    start = text.find(f"\n{heading}\n")
    if start < 0:
        problems.append(f"docs/plan.md has no section {heading!r}")
        return ""
    body = text[start + 1 + len(heading) + 1 :]
    following = re.search(r"^## ", body, re.MULTILINE)
    return body[: following.start()] if following else body


def spans(text):
    flat = re.sub(r"\s+", " ", text)
    return re.findall(r"`([^`]+)`", flat)


with open("docs/plan.md") as handle:
    plan = handle.read()

checklist = section(plan, "## M1 exit checklist")
TEST_NAME = re.compile(r"^[a-z][a-z0-9]*(_[a-z0-9]+){3,}$")
tests = sorted({span for span in spans(checklist) if TEST_NAME.match(span)})
if not tests:
    problems.append("the M1 exit checklist cites no test name")
for test in tests:
    result = subprocess.run(
        ["grep", "-rlE", "--include=*.rs", f"fn {test}\\b", "crates", "products"],
        check=False,
        capture_output=True,
    )
    if result.returncode != 0:
        problems.append(f"the M1 exit checklist cites the test {test}, which is not defined under crates/ or products/")
print(f"{len(tests)} test names of the M1 exit checklist looked up")


def run_commands(path):
    commands = []
    with open(path) as handle:
        lines = handle.read().splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        match = re.match(r"^(\s*)run:\s*(.*)$", line)
        index += 1
        if not match:
            continue
        indent, value = match.groups()
        if value in ("|", ">", "|-", ">-"):
            while index < len(lines) and (not lines[index].strip() or len(lines[index]) - len(lines[index].lstrip()) > len(indent)):
                if lines[index].strip():
                    commands.append(lines[index].strip())
                index += 1
        else:
            commands.append(value.strip())
    return commands


ci = run_commands(".github/workflows/ci.yml")
ci_text = "\n".join(ci)

commands = [span for span in spans(checklist) if span.startswith(("hack/", "cargo ", "env "))]
for command in commands:
    if command.startswith("hack/"):
        script = command.split()[0]
        if not os.access(script, os.X_OK):
            problems.append(f"the M1 exit checklist cites {command}, but {script} is not an executable file under hack/")
    else:
        prefix = command.split("<", 1)[0].rstrip()
        if prefix not in ci_text:
            problems.append(f"the M1 exit checklist cites {command}, which is not a run command of .github/workflows/ci.yml")
print(f"{len(commands)} commands of the M1 exit checklist looked up")

with open("README.md") as handle:
    readme = handle.read()
marker = "The checks below are the ones CI runs"
start = readme.find(marker)
if start < 0:
    problems.append("README.md has no check list introduced by 'The checks below are the ones CI runs'")
    listed = []
else:
    block = re.search(r"```\n(.*?)```", readme[start:], re.DOTALL)
    listed = [line.strip() for line in block.group(1).splitlines() if line.strip() and not line.startswith("export ")] if block else []
    if not listed:
        problems.append("the README's check list is empty")

how = spans(section(plan, "## How we work"))
for command in listed:
    if command not in ci:
        problems.append(f"the README lists {command}, which is not a run command of .github/workflows/ci.yml")
    if command not in how:
        problems.append(f"the README lists {command}, which the section 'How we work' of docs/plan.md does not name")
print(f"{len(listed)} commands of the README's check list compared with ci.yml and 'How we work'")

for problem in problems:
    print(problem, file=sys.stderr)
if problems:
    sys.exit(1)
print("docs checked")
EOF
