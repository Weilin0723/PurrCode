#!/usr/bin/env python3
"""Render the FSE main-track result figures from the frozen analysis JSON."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Mapping

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


B_COLOR = "#c44e52"
C_COLOR = "#4c72b0"
ACCENT = "#55a868"
GRID = "#d9d9d9"


def load(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise SystemExit(f"{path}: expected an object")
    return value


def style(ax) -> None:
    ax.grid(axis="y", color=GRID, linewidth=0.8, alpha=0.8)
    ax.set_axisbelow(True)
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)


def endpoint(analysis: Mapping[str, Any], name: str) -> Mapping[str, Any]:
    value = analysis["endpoints"][name]
    if not isinstance(value, Mapping):
        raise SystemExit(f"analysis.endpoints.{name} is not an object")
    return value


def render_false_ready(analysis: Mapping[str, Any], out: Path) -> None:
    report = endpoint(analysis, "false_ready")
    fig, ax = plt.subplots(figsize=(4.7, 3.1))
    values = [100.0 * float(report["B"]), 100.0 * float(report["C"])]
    bars = ax.bar(["B (no gate)", "C (binding gate)"], values, color=[B_COLOR, C_COLOR], width=0.62)
    for bar, value in zip(bars, values):
        ax.text(bar.get_x() + bar.get_width() / 2, value + 1.5, f"{value:.1f}%", ha="center", fontsize=10)
    ax.set_ylabel("externally false-ready (%)")
    ax.set_ylim(0, max(values) + 12)
    interval = report["repository_cluster_bootstrap_interval"]
    ax.set_title(
        f"C - B = {100.0 * float(report['difference']):.1f} pp\n"
        f"repo-cluster 95% CI [{100.0 * float(interval[0]):.1f}, {100.0 * float(interval[1]):.1f}]",
        fontsize=11,
    )
    style(ax)
    fig.tight_layout()
    fig.savefig(out.with_suffix(".png"), dpi=220)
    fig.savefig(out.with_suffix(".pdf"))
    plt.close(fig)


def render_guardrail(analysis: Mapping[str, Any], out: Path) -> None:
    report = endpoint(analysis, "correct_delivery")
    fig, ax = plt.subplots(figsize=(4.7, 3.1))
    values = [100.0 * float(report["B"]), 100.0 * float(report["C"])]
    bars = ax.bar(["B (no gate)", "C (binding gate)"], values, color=[B_COLOR, C_COLOR], width=0.62)
    for bar, value in zip(bars, values):
        ax.text(bar.get_x() + bar.get_width() / 2, value + 1.5, f"{value:.1f}%", ha="center", fontsize=10)
    ax.set_ylabel("correct delivery (%)")
    ax.set_ylim(0, max(values) + 12)
    interval = report["repository_cluster_bootstrap_interval"]
    ax.set_title(
        f"C - B = {100.0 * float(report['difference']):.1f} pp\n"
        f"repo-cluster 95% CI [{100.0 * float(interval[0]):.1f}, {100.0 * float(interval[1]):.1f}]",
        fontsize=11,
    )
    style(ax)
    fig.tight_layout()
    fig.savefig(out.with_suffix(".png"), dpi=220)
    fig.savefig(out.with_suffix(".pdf"))
    plt.close(fig)


def render_repository_effects(analysis: Mapping[str, Any], out: Path) -> None:
    report = endpoint(analysis, "false_ready")
    repositories = report["repository_differences"]
    names = sorted(repositories)
    values = [100.0 * float(repositories[name]) for name in names]
    order = np.argsort(values)
    names = [names[index] for index in order]
    values = [values[index] for index in order]
    fig, ax = plt.subplots(figsize=(7.2, 4.4))
    colors = [ACCENT if value < 0 else B_COLOR for value in values]
    ax.barh(names, values, color=colors, height=0.62)
    ax.axvline(0, color="#333333", linewidth=0.9)
    ax.set_xlabel("C - B false-ready (percentage points)")
    ax.set_title("Repository-level false-ready effect", fontsize=11)
    ax.grid(axis="x", color=GRID, linewidth=0.8, alpha=0.8)
    ax.set_axisbelow(True)
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    fig.tight_layout()
    fig.savefig(out.with_suffix(".png"), dpi=220)
    fig.savefig(out.with_suffix(".pdf"))
    plt.close(fig)


def render_attrition(analysis: Mapping[str, Any], out: Path) -> None:
    attrition = analysis["attrition"]
    cells = []
    labels = []
    values = []
    for variant in ("static", "correction"):
        for arm in ("B", "C"):
            key = f"{variant}:{arm}"
            row = attrition[key]
            total = int(row["total"])
            invalid = total - int(row["valid"])
            cells.append((variant, arm, invalid, total))
            labels.append(f"{variant}\n{arm}")
            values.append(100.0 * invalid / total if total else 0.0)
    fig, ax = plt.subplots(figsize=(5.5, 3.1))
    bars = ax.bar(labels, values, color=[B_COLOR, C_COLOR, B_COLOR, C_COLOR], width=0.62)
    for bar, value in zip(bars, values):
        ax.text(bar.get_x() + bar.get_width() / 2, value + 0.3, f"{value:.1f}%", ha="center", fontsize=9)
    ax.set_ylabel("non-valid scheduled runs (%)")
    ax.set_title("Operational attrition", fontsize=11)
    style(ax)
    fig.tight_layout()
    fig.savefig(out.with_suffix(".png"), dpi=220)
    fig.savefig(out.with_suffix(".pdf"))
    plt.close(fig)


def render_component_compare(e2e: Mapping[str, Any], component: Mapping[str, Any] | None, out: Path) -> None:
    fig, ax = plt.subplots(figsize=(5.7, 3.2))
    labels = ["E2E correction"]
    values = [100.0 * float(endpoint(e2e, "false_ready")["difference"])]
    colors = [C_COLOR]
    if component is not None:
        labels.append("Component study")
        values.append(100.0 * float(endpoint(component, "false_ready")["difference"]))
        colors.append(ACCENT)
    bars = ax.bar(labels, values, color=colors, width=0.55)
    ax.axhline(0, color="#333333", linewidth=0.9)
    for bar, value in zip(bars, values):
        ax.text(bar.get_x() + bar.get_width() / 2, value - 3.0 if value < 0 else value + 1.5, f"{value:.1f} pp", ha="center", fontsize=10)
    ax.set_ylabel("C - B false-ready (percentage points)")
    ax.set_title("Component study vs repository-disjoint E2E", fontsize=11)
    style(ax)
    fig.tight_layout()
    fig.savefig(out.with_suffix(".png"), dpi=220)
    fig.savefig(out.with_suffix(".pdf"))
    plt.close(fig)


def write_table(analysis: Mapping[str, Any], path: Path) -> None:
    rows = []
    for name in ("false_ready", "correct_delivery"):
        report = endpoint(analysis, name)
        interval = report["repository_cluster_bootstrap_interval"]
        rows.append(
            f"| {name} | {100*float(report['B']):.1f}% | {100*float(report['C']):.1f}% | "
            f"{100*float(report['difference']):.1f} pp | "
            f"[{100*float(interval[0]):.1f}, {100*float(interval[1]):.1f}] | "
            f"{report['repository_sign_test_p_two_sided']:.5f} |"
        )
    path.write_text(
        "| Endpoint | B | C | C - B | repo-cluster 95% CI | sign p |\n"
        "| --- | ---: | ---: | ---: | ---: | ---: |\n"
        + "\n".join(rows)
        + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("analysis", type=Path)
    parser.add_argument("--out", type=Path, default=Path("docs/research/figures"))
    parser.add_argument("--component-analysis", type=Path)
    args = parser.parse_args()
    analysis = load(args.analysis)
    component = load(args.component_analysis) if args.component_analysis else None
    args.out.mkdir(parents=True, exist_ok=True)
    render_false_ready(analysis, args.out / "fig1-false-ready")
    render_guardrail(analysis, args.out / "fig2-correct-delivery")
    render_repository_effects(analysis, args.out / "fig3-repository-effects")
    render_attrition(analysis, args.out / "fig4-attrition")
    render_component_compare(analysis, component, args.out / "fig5-component-vs-e2e")
    write_table(analysis, args.out / "main-results.md")
    print(f"wrote figures to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
