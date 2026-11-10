#!/usr/bin/env python3
"""decision.txt agrees with the typed verdict the jev call actually returned.

The old oracle pinned `DELETE`, and the fixture argues for deletion on every line - so
an agent that never called jev, or that got `KEEP` and ignored it, passed exactly as a
correct one did (review #39). The task's own instruction is "act on the answer", so the
thing to grade is the AGREEMENT between the tool's verdict and the file.

A `jev check` answer is a calibrated probability of "yes"; the yes/no split is the
probability's own midpoint. An agent with no successful jev call fails: the task asked
for the typed path, and a decision reached any other way is the behaviour it catches.
"""
import json
import os
import re
import sys

sys.path.insert(0, os.environ.get("QA_SCRIPTS_DIR",
                                  os.path.join(os.path.dirname(__file__), "..", "..", "scripts", "qa")))
import qa_oracle

work = os.environ.get("QA_WORK", "")
path = os.path.join(work, "decision.txt")
if not os.path.isfile(path):
    print("decision.txt missing")
    sys.exit(1)
text = open(path, encoding="utf-8").read().strip().upper()
if text not in ("DELETE", "KEEP"):
    print(f"decision.txt holds {text!r}, expected DELETE or KEEP")
    sys.exit(1)

rows = qa_oracle.load_rows(os.environ.get("QA_TRANSCRIPT", ""))
calls = qa_oracle.successful_calls(rows, "jev")
if not calls:
    print("no successful jev call: the decision was not reached through the typed tool")
    sys.exit(1)

# The decision the task asks for is whether this service should be DELETED now. A jev
# call about anything else (how long the README is, how urgent the ticket is) is not an
# answer to it, so its probability cannot license either word in decision.txt (#39).
# The stems are how the question can be phrased; an inverted phrasing ("should we keep
# it?") flips which word p(yes) licenses.
DELETE_STEMS = ("delet", "remov", "retire", "decommission", "deprecat")
KEEP_STEMS = ("keep", "retain", "preserv")


def asked_questions(call: dict) -> list[tuple[str, str]]:
    """`(answer id, question text)` pairs the call asked, from its own arguments."""
    arguments = json.loads(call["arguments"])
    questions = []
    if isinstance(arguments.get("questions"), list):
        for question in arguments["questions"]:
            if not isinstance(question, dict):
                continue
            questions.append((
                str(question.get("id") or ""),
                " ".join(str(question.get(key) or "")
                         for key in ("instructions", "question", "true", "false")),
            ))
    else:
        questions.append((
            str(arguments.get("id") or arguments.get("action") or "check"),
            " ".join(str(arguments.get(key) or "")
                     for key in ("question", "instructions", "true", "false")),
        ))
    return questions


def probability(call: dict, answer_id: str) -> float | None:
    """The noul answer under `answer_id`: the structured detail, else the text line."""
    result = qa_oracle.result_of(rows, call)
    answers = (result.get("details") or {}).get("answers") or {}
    answer = answers.get(answer_id)
    if isinstance(answer, dict) and isinstance(answer.get("noul"), (int, float)):
        return float(answer["noul"])
    # The rendered line is `<id>: <p>` (`tool.rs::render_result`): match the id that was
    # asked, never any line.
    match = re.search(
        rf"^{re.escape(answer_id)}:\s*(0\.\d+|1(?:\.0+)?)\s*$",
        result.get("text", ""),
        re.MULTILINE,
    )
    return float(match.group(1)) if match else None


def names_a_fixture_file(question: str) -> bool:
    """Whether the question is about a FILE in the work tree rather than the service.

    A deletion stem alone is not enough (review #39, residual): `should we delete the
    README?` carries `delet`, and licensing DELETE from it decided a question the task
    never asked — deleting a file is not deleting the service. The file names come from the
    fixture itself (what `setup.sh` put in the work tree), so nothing is hardcoded here.
    """
    lowered = question.lower()
    work = os.environ.get("QA_WORK", "")
    try:
        entries = os.listdir(work) if work else []
    except OSError:
        entries = []
    for entry in entries:
        stem = entry.rsplit(".", 1)[0].lower()
        if len(stem) >= 3 and stem in lowered:
            return True
    return False


verdicts = []
for call in calls:
    for answer_id, question in asked_questions(call):
        lowered = question.lower()
        if names_a_fixture_file(question):
            continue
        if any(stem in lowered for stem in KEEP_STEMS) and not any(
            stem in lowered for stem in DELETE_STEMS
        ):
            answers_keep = True
        elif any(stem in lowered for stem in DELETE_STEMS):
            answers_keep = False
        else:
            continue
        value = probability(call, answer_id)
        if value is not None:
            verdicts.append((value, answers_keep))
if not verdicts:
    asked = [question for call in calls for _, question in asked_questions(call)]
    print(
        "no jev call asked whether this service should be deleted: the typed question "
        f"the task names was never put to the tool (it asked {asked})")
    sys.exit(1)

def resolved(value: float, answers_keep: bool) -> str:
    """The verdict one answer licenses: p(yes) >= 0.5 agrees with the question's direction."""
    return "KEEP" if (value >= 0.5) == answers_keep else "DELETE"


# EVERY answer about this service has to agree. `verdicts[-1]` let an agent ask twice and
# act on whichever answer suited it while ignoring the other (review #39, residual).
licensed = [(value, answers_keep, resolved(value, answers_keep))
            for value, answers_keep in verdicts]
if len({verdict for _, _, verdict in licensed}) > 1:
    asked = "; ".join(
        f"p(yes)={value:.2f} -> {verdict}" for value, _, verdict in licensed)
    print(
        "jev was asked about this service more than once and the answers license different "
        f"decisions ({asked}): acting on only one of them decides past the tool")
    sys.exit(1)
verdict, answers_keep, _ = licensed[-1]
# p(yes) >= 0.5 agrees with the direction the question asks about.
yes = verdict >= 0.5
expected = "KEEP" if yes == answers_keep else "DELETE"
if text != expected:
    asked = "keep it?" if answers_keep else "delete it now?"
    print(
        f"decision.txt holds {text}, but jev answered p(yes)={verdict:.2f} "
        f"({expected}) to {asked!r}: the task asked the agent to ACT on the verdict, "
        "not to decide past it")
    sys.exit(1)
