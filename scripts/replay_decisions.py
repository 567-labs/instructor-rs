"""Synthetic responses to the ticket in Python's test_live_decisions.py.

Validate the outgoing contract against the fixture before returning an answer.
The values exercise client decoding, score scaling and raw metadata handling.
"""

import json

MODELS = {
    "openai": "gpt-6-luna",
    "openrouter": "typesafe/jev-1.13",
    "typesafe": "jev-latest",
}
STATE = {
    "ticket": "Please refund the duplicate charge on my invoice. There is no rush."
}
DEPARTMENT = "Which team handles the customer's request?"
REFUND = "Is the customer requesting a refund?"
URGENCY = "How urgently does the customer need help?"
LEVELS = ["Not urgent", "Somewhat urgent", "Emergency"]


def _check_request(provider, body):
    if provider not in MODELS:
        raise ValueError("unsupported Decisions route")
    if not isinstance(body, dict) or body.get("model") != MODELS[provider]:
        raise ValueError(f"{provider} Decisions requires model {MODELS[provider]}")
    if provider == "openai":
        if set(body) != {"model", "input", "questions"}:
            raise ValueError("expected model, input and questions for OpenAI Decisions")
        if not isinstance(body["input"], str) or json.loads(body["input"]) != STATE:
            raise ValueError(
                "OpenAI input must contain the ticket encoded as JSON text"
            )
        questions = [
            {
                "name": "department",
                "type": "choice",
                "instructions": DEPARTMENT,
                "choices": [{"value": "billing"}, {"value": "technical"}],
            },
            {"name": "refund", "type": "predicate", "instructions": REFUND},
            {
                "name": "urgency",
                "type": "score",
                "instructions": URGENCY,
                "levels": [
                    {"label": str(index), "description": level}
                    for index, level in enumerate(LEVELS)
                ],
            },
        ]
    else:
        if set(body) != {"model", "state", "questions"} or body["state"] != STATE:
            raise ValueError(f"{provider} Jev state must contain the original ticket")
        questions = {
            "department": {
                "type": "choice",
                "instructions": DEPARTMENT,
                "criteria": {"billing": None, "technical": None},
            },
            "refund": {"type": "noul", "instructions": REFUND},
            "urgency": {"type": "score", "instructions": URGENCY, "criteria": LEVELS},
        }
    if body["questions"] != questions:
        raise ValueError(
            f"{provider} Decisions questions differ from the ticket contract"
        )


def decision_response(provider, body):
    _check_request(provider, body)

    department = {
        "type": "choice",
        "choice": "billing",
        "confidence": 0.8,
        "probabilities": {"billing": 0.9, "technical": 0.1},
    }
    urgency = {
        "type": "score",
        "score": 0.4,
        "confidence": 0.65,
        "probabilities": {"0": 0.7, "1": 0.2, "2": 0.1},
    }
    if provider == "openai":
        for answer in (department, urgency):
            answer["probabilities"] = [
                {"value": label, "probability": probability}
                for label, probability in answer["probabilities"].items()
            ]
        answers = [
            {"name": "department", **department},
            {"name": "refund", "type": "predicate", "probability": 0.95},
            {"name": "urgency", **urgency},
        ]
        usage = {"input_tokens": 100, "output_tokens": 0}
    else:
        urgency["legend"] = {str(index): level for index, level in enumerate(LEVELS)}
        answers = {
            "department": department,
            "refund": {"type": "noul", "noul": 0.95},
            "urgency": urgency,
        }
        usage = {"input_tokens": 100, "output_tokens": 3}
        if provider == "openrouter":
            usage["cost"] = 0.001
    return {"model": MODELS[provider], "answers": answers, "usage": usage}
