#!/usr/bin/env python3
"""
PostToolUse hook — keeps docs/TODO.md in sync with Claude Code task state.

TaskCreate  → records taskId→subject in .claude/task-map.json
TaskUpdate  → toggles checkbox in docs/TODO.md:
                completed  → [x]
                in_progress → [>]
                pending    → [ ]
"""
import json, re, sys, pathlib

ROOT     = pathlib.Path(__file__).resolve().parent.parent
MAP_FILE = ROOT / ".claude" / "task-map.json"
TODO     = ROOT / "docs" / "TODO.md"


def load_map() -> dict:
    return json.loads(MAP_FILE.read_text()) if MAP_FILE.exists() else {}


def save_map(m: dict) -> None:
    MAP_FILE.write_text(json.dumps(m, indent=2) + "\n")


def subject_to_pattern(subject: str) -> str:
    """Return a regex that loosely matches a TODO line for this subject."""
    # strip backtick formatting, collapse whitespace, escape for regex
    core = re.sub(r'`', '', subject).strip()
    # allow any checkbox prefix and trailing text
    return re.escape(core)


STATUS_MARKS = {
    "completed":   "[x]",
    "in_progress": "[>]",
    "pending":     "[ ]",
}

MARK_RE = re.compile(r'- \[[ >x]\] ')


def update_todo(subject: str, status: str) -> None:
    if not TODO.exists():
        return
    mark = STATUS_MARKS.get(status)
    if not mark:
        return

    pat = subject_to_pattern(subject)
    lines = TODO.read_text().splitlines(keepends=True)
    changed = False
    for i, line in enumerate(lines):
        if MARK_RE.match(line) and re.search(pat, line, re.IGNORECASE):
            lines[i] = MARK_RE.sub(f"- {mark} ", line, count=1)
            changed = True
            break

    if changed:
        TODO.write_text("".join(lines))


def main() -> None:
    try:
        hook = json.load(sys.stdin)
    except Exception:
        return

    tool = hook.get("tool_name", "")
    inp  = hook.get("tool_input", {})

    if tool == "TaskCreate":
        subject = inp.get("subject", "").strip()
        if not subject:
            return
        resp_text = str(hook.get("tool_response", ""))
        m = re.search(r"Task #(\d+)", resp_text)
        if m:
            task_map = load_map()
            task_map[m.group(1)] = subject
            save_map(task_map)

    elif tool == "TaskUpdate":
        status  = inp.get("status", "")
        task_id = str(inp.get("taskId", ""))
        if not status or not task_id:
            return
        task_map = load_map()
        subject  = task_map.get(task_id, "")
        if subject:
            update_todo(subject, status)


if __name__ == "__main__":
    main()
