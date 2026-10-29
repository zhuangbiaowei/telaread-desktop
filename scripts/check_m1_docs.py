#!/usr/bin/env python3
"""核对 M1 文档口径与代码事实是否一致（doc ⇄ code 双向校验）。

校验对象与真相来源（唯一真相 = 代码 / 上游 R2）：

| 校验项 | 文档侧 | 代码侧（真相） |
|---|---|---|
| 17 张同步行表键名 | `docs/m1_acceptance.md` 的 `check:table-keys` 块 | `crates/tela-proto/src/rows.rs::SyncRows::TABLE_NAMES` |
| 协议契约常量 | `check:constants` 块 | `crates/tela-proto/src/lib.rs`、`crates/tela-sync/src/pull.rs` 的 `pub const` |
| 错误码（机器码 + 退出码） | `check:error-codes` 块 | `crates/tela-proto/src/error.rs` 的 `code()` / `exit_code()` |
| 拉取层失败阶段 | `check:pull-stages` 块 | `crates/tela-sync/src/pull.rs::PullStage` |
| 开发计划/协议提案里的 17 张表述 | `docs/development_plan.md` §2.2/§6 C2/§8.1、`docs/api-proposal.md` §7.1 | 同上 `TABLE_NAMES` |

用法：`python3 scripts/check_m1_docs.py`；任一项不一致即非 0 退出（便于作为验收 check）。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

ROWS_RS = "crates/tela-proto/src/rows.rs"
PROTO_LIB_RS = "crates/tela-proto/src/lib.rs"
PULL_RS = "crates/tela-sync/src/pull.rs"
ERROR_RS = "crates/tela-proto/src/error.rs"
M1_DOC = "docs/m1_acceptance.md"
PLAN = "docs/development_plan.md"
API = "docs/api-proposal.md"

# 代码侧必须存在的常量名（顺序即文档块顺序）
PROTO_CONSTANTS = [
    "MIN_CLIENT",
    "CLIENT_HEADER",
    "MEMBER_HEADER",
    "PAGE_ROWS",
    "HORIZON_DAYS",
    "TITLE_IDS_MAX",
    "MAX_MUTATIONS_PER_PUSH",
]
PULL_CONSTANTS = ["DEFAULT_MAX_ATTEMPTS", "DEFAULT_MAX_PAGES"]

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)
    print(f"FAIL {message}")


def read(rel: str) -> str:
    path = ROOT / rel
    if not path.is_file():
        fail(f"{rel} 不存在（验收产物缺失）")
        return ""
    text = path.read_text(encoding="utf-8")
    if not text.strip():
        fail(f"{rel} 为空")
    return text


def marker_block(text: str, rel: str, name: str) -> str:
    begin, end = f"<!-- check:{name} -->", f"<!-- /check:{name} -->"
    if begin not in text or end not in text:
        fail(f"{rel} 缺少标记块 {begin} … {end}")
        return ""
    return text.split(begin, 1)[1].split(end, 1)[0]


def parse_table_names(source: str) -> list[str]:
    m = re.search(r"pub const TABLE_NAMES:\s*\[&'static str;\s*(\d+)\]\s*=\s*\[(.*?)\];", source, re.S)
    if not m:
        fail(f"{ROWS_RS} 未找到 TABLE_NAMES 常量")
        return []
    declared, body = int(m.group(1)), m.group(2)
    names = re.findall(r'"([^"]+)"', body)
    if declared != len(names):
        fail(f"TABLE_NAMES 声明长度 {declared} 与实际条目 {len(names)} 不一致")
    if len(names) != 17:
        fail(f"TABLE_NAMES 应为 17 张表，实际 {len(names)}")
    return names


def parse_constants(source: str, names: list[str], rel: str) -> dict[str, str]:
    found: dict[str, str] = {}
    for name in names:
        m = re.search(rf"pub const {name}:\s*[^=;]+=\s*([^;]+);", source)
        if not m:
            fail(f"{rel} 缺少常量 {name}")
            continue
        value = m.group(1).strip().strip('"').replace("_", "")
        found[name] = value
    return found


def parse_error_maps(source: str) -> tuple[dict[str, str], dict[str, str], list[str]]:
    enum_body = re.search(r"pub enum TelaError\s*\{(.*?)\n\}", source, re.S)
    variants: list[str] = []
    if enum_body:
        variants = re.findall(r"^\s{4}([A-Z]\w+)\s*\{", enum_body.group(1), re.M)

    code_fn = re.search(r"pub fn code\(&self\)\s*->\s*&'static str\s*\{(.*?)\n    \}", source, re.S)
    codes: dict[str, str] = {}
    if code_fn:
        for variant, code in re.findall(r"Self::(\w+)\s*\{\s*\.\.\s*\}\s*=>\s*\"([^\"]+)\"", code_fn.group(1)):
            codes[variant] = code

    exit_fn = re.search(r"pub fn exit_code\(&self\)\s*->\s*i32\s*\{(.*?)\n    \}", source, re.S)
    exits: dict[str, str] = {}
    if exit_fn:
        for arm, code in re.findall(
            r"((?:Self::\w+\s*\{\s*\.\.\s*\}(?:\s*\|\s*)?)+)\s*=>\s*(\d+)", exit_fn.group(1)
        ):
            for variant in re.findall(r"Self::(\w+)", arm):
                exits[variant] = code

    if not variants:
        fail(f"{ERROR_RS} 未解析出 TelaError 变体")
    return codes, exits, variants


def parse_pull_stages(source: str) -> list[str]:
    body = re.search(r"pub enum PullStage\s*\{(.*?)\n\}", source, re.S)
    if not body:
        fail(f"{PULL_RS} 未找到 PullStage")
        return []
    return re.findall(r"^\s{4}([A-Z]\w+)\s*\{", body.group(1), re.M)


def main() -> int:
    rows_src = read(ROWS_RS)
    proto_src = read(PROTO_LIB_RS)
    pull_src = read(PULL_RS)
    error_src = read(ERROR_RS)
    doc = read(M1_DOC)
    plan = read(PLAN)
    api = read(API)

    # ---- 代码侧事实 ----
    table_names = parse_table_names(rows_src)
    proto_consts = parse_constants(proto_src, PROTO_CONSTANTS, PROTO_LIB_RS)
    pull_consts = parse_constants(pull_src, PULL_CONSTANTS, PULL_RS)
    codes, exits, variants = parse_error_maps(error_src)
    stages = parse_pull_stages(pull_src)

    for variant in variants:
        if variant not in codes:
            fail(f"{ERROR_RS} 变体 {variant} 缺少机器码 code()")
        if variant not in exits:
            fail(f"{ERROR_RS} 变体 {variant} 缺少退出码 exit_code()")

    # ---- 文档侧：marker 块必须与代码逐字一致 ----
    key_block = marker_block(doc, M1_DOC, "table-keys")
    doc_names = re.findall(r"`([A-Za-z]+)`", key_block)
    if doc_names != table_names:
        fail(f"{M1_DOC} 表键名与 TABLE_NAMES 不一致：doc={doc_names} code={table_names}")
    if "17" not in key_block:
        fail(f"{M1_DOC} 表键区块未写明 17 张")

    const_block = marker_block(doc, M1_DOC, "constants")
    doc_consts = dict(re.findall(r"^([A-Z][A-Z0-9_]*)=(.*)$", const_block, re.M))
    expected = {**proto_consts, **pull_consts}
    for name, value in expected.items():
        if name not in doc_consts:
            fail(f"{M1_DOC} 常量块缺少 {name}={value}")
        elif doc_consts[name].strip().replace("_", "") != value:
            fail(f"{M1_DOC} 常量 {name} 值不一致：doc={doc_consts[name].strip()} code={value}")
    extra = set(doc_consts) - set(expected)
    if extra:
        fail(f"{M1_DOC} 常量块出现代码中不存在的名字：{sorted(extra)}")

    code_block = marker_block(doc, M1_DOC, "error-codes")
    doc_codes: dict[str, tuple[str, str]] = {}
    for line in code_block.strip().splitlines():
        parts = line.split()
        if len(parts) == 3:
            doc_codes[parts[0]] = (parts[1], parts[2])
        elif line.strip():
            fail(f"{M1_DOC} 错误码块行格式非法（应为 `变体 机器码 退出码`）：{line!r}")
    if set(doc_codes) != set(variants):
        fail(f"{M1_DOC} 错误码块变体集合不一致：doc={sorted(doc_codes)} code={sorted(variants)}")
    for variant, (machine, exit_code) in doc_codes.items():
        if variant in codes and machine != codes[variant]:
            fail(f"{M1_DOC} {variant} 机器码不一致：doc={machine} code={codes[variant]}")
        if variant in exits and exit_code != exits[variant]:
            fail(f"{M1_DOC} {variant} 退出码不一致：doc={exit_code} code={exits[variant]}")

    stage_block = marker_block(doc, M1_DOC, "pull-stages")
    doc_stages = re.findall(r"`([A-Za-z]+)`", stage_block)
    if doc_stages != stages:
        fail(f"{M1_DOC} PullStage 不一致：doc={doc_stages} code={stages}")

    # ---- 开发计划 / 协议提案：17 张表述必须与代码一致 ----
    for name in table_names:
        if f"`{name}`" not in plan:
            fail(f"{PLAN} 未出现表键名 `{name}`")
        if f"| `{name}` |" not in api:
            fail(f"{API} §7.1 表清单未出现 `{name}`")
    for stale in ("同步表的 15 个键名", "全集记作 15 张表全部", "15 张表键名"):
        if stale in plan:
            fail(f"{PLAN} 仍有旧口径残留：{stale!r}")

    # C2 的 SyncRows 字段清单必须包含两个后补表
    c2 = re.search(r"### C2 `tela-proto::SyncRows` 缺表容错(.*?)###", plan, re.S)
    if not c2:
        fail(f"{PLAN} 未找到 §6 C2 小节")
    else:
        body = c2.group(1)
        for name in table_names:
            if name not in body:
                fail(f"{PLAN} §6 C2 的 SyncRows 清单缺少 `{name}`")

    if failures:
        print(f"\n共 {len(failures)} 项不一致")
        return 1
    print(
        "OK 文档口径与代码一致："
        f"行表 {len(table_names)} 张、常量 {len(expected)} 个、错误变体 {len(variants)} 个、拉取阶段 {len(stages)} 个"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
