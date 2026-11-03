"""Tests for the census client scenarios (packet CEN-4, `docs/lanes/cen4.md`).

The scenarios run only inside the client, under Stackless Python 2.7, so
nothing here runs one. What can be checked without a client is checked:

- each carries the canonical prelude (what `sync-prelude.py --check` checks),
  and the census block is byte-identical across all six;
- each body parses, and uses nothing Python 2.7 lacks;
- `census_canary` fires exactly the `by: client` rows of
  `tools/census_canary.py`, with exactly their targets;
- every address or name any of them reaches is unownable (TEST-NET-1, the
  IPv6 documentation prefix, `.invalid`), and none names a real domain;
- each refuses before it does anything, outside a census;
- each one's declared timeout covers its own waits.

Stdlib unittest; reads files only.
"""

import ast
import importlib.util
import ipaddress
import pathlib
import re
import sys
import textwrap
import unittest
import urllib.parse

TOOLS = pathlib.Path(__file__).resolve().parent
SCEN = TOOLS / "client-ci" / "scenarios"
NAMES = ("census_canary", "census_idle", "census_links", "census_cc",
         "census_hang", "census_crash",
         # lane PHONE-HOME (issue #40): the paths the first six never reach
         "census_report_bug", "census_urls", "census_cdn")
BLOCK_START = "        # -- census block:"
BLOCK_END = "        # -- end of the census block"
#: Spare for login, the undock and the quit, on top of the scenario's waits.
LOGIN_SPARE_S = 120


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


if str(TOOLS) not in sys.path:
    sys.path.append(str(TOOLS))
sync = _load("sync_prelude", TOOLS / "client-ci" / "sync-prelude.py")
canary = _load("census_canary", TOOLS / "census_canary.py")


def _text(name):
    return (SCEN / f"{name}.py").read_text()


def _parts(name):
    return sync.split(_text(name), name)


def _body_tree(name):
    """The scenario body (`def eoci_body` and the marker comment), dedented
    out of the prelude's function and parsed."""
    body = _parts(name)[2]
    tree = ast.parse(textwrap.dedent(body), filename=f"{name}.py:body")
    fns = [n for n in tree.body if isinstance(n, ast.FunctionDef)]
    assert [f.name for f in fns] == ["eoci_body"], [f.name for f in fns]
    return fns[0]


def _strings(node):
    return [n.value for n in ast.walk(node)
            if isinstance(n, ast.Constant) and isinstance(n.value, str)]


def _assigned(fn):
    """Top-level `NAME = <literal>` in the body, evaluated."""
    out = {}
    for st in fn.body:
        if isinstance(st, ast.Assign) and len(st.targets) == 1 \
                and isinstance(st.targets[0], ast.Name):
            if any(isinstance(n, (ast.Name, ast.Call, ast.Attribute))
                   for n in ast.walk(st.value)):
                continue
            expr = ast.Expression(st.value)
            out[st.targets[0].id] = eval(compile(expr, "<const>", "eval"),
                                         {"__builtins__": {}})
    return out


class Prelude(unittest.TestCase):
    def test_every_census_scenario_carries_the_prelude(self):
        _, part1, _, part2 = sync.split((SCEN / "_prelude.py").read_text(), "_prelude")
        for name in NAMES:
            with self.subTest(name):
                _, had1, _, had2 = _parts(name)
                self.assertEqual(had1, part1)
                self.assertEqual(had2, part2)

    def test_the_census_block_is_identical_in_all(self):
        blocks = {}
        for name in NAMES:
            body = _parts(name)[2]
            start, end = body.index(BLOCK_START), body.index(BLOCK_END)
            blocks[name] = body[start:end]
        self.assertEqual(len(set(blocks.values())), 1, sorted(blocks))

    def test_each_names_itself_and_declares_a_timeout(self):
        for name in NAMES:
            with self.subTest(name):
                head = _parts(name)[0]
                self.assertIn(f"EOCI_SCENARIO = '{name}'\n", head)
                self.assertRegex(head, r"(?m)^EOCI_CENSUS_TIMEOUT_S = \d+$")


class Python27(unittest.TestCase):
    BANNED = (ast.JoinedStr, ast.Nonlocal, ast.YieldFrom, ast.AsyncFunctionDef,
              ast.Await, ast.NamedExpr, ast.AnnAssign)

    def test_bodies_parse_and_use_nothing_27_lacks(self):
        for name in NAMES:
            with self.subTest(name):
                fn = _body_tree(name)
                for node in ast.walk(fn):
                    self.assertNotIsInstance(node, self.BANNED, ast.dump(node)[:80])
                    if isinstance(node, (ast.FunctionDef, ast.Lambda)):
                        self.assertFalse(node.args.kwonlyargs, name)
                        self.assertFalse(node.args.posonlyargs, name)
                    if isinstance(node, ast.BinOp):
                        self.assertNotIsInstance(node.op, ast.MatMult)

    def test_no_print_statement_or_function(self):
        # `print` differs between the two; the client log is `say`.
        for name in NAMES:
            with self.subTest(name):
                calls = [n.func.id for n in ast.walk(_body_tree(name))
                         if isinstance(n, ast.Call) and isinstance(n.func, ast.Name)]
                self.assertNotIn("print", calls)


class Canary(unittest.TestCase):
    def setUp(self):
        self.fn = _body_tree("census_canary")
        self.consts = _assigned(self.fn)
        self.client = {cid: row for cid, row in canary.CANARIES.items()
                       if row["by"] == "client"}

    def test_it_fires_exactly_the_client_rows(self):
        self.assertEqual(set(self.consts["ROW_IDS"]), set(self.client))
        self.assertEqual(len(self.consts["ROW_IDS"]), len(self.client))
        rows = next(st.value for st in self.fn.body
                    if isinstance(st, ast.Assign) and st.targets[0].id == "rows")
        self.assertEqual({k.value for k in rows.keys}, set(self.client))

    def test_every_client_target_is_in_the_scenario(self):
        strings = _strings(self.fn)
        for cid, row in self.client.items():
            for target in row["targets"]:
                with self.subTest(cid, target=target):
                    self.assertTrue(any(target in s for s in strings),
                                    f"{target} not fired by the scenario")

    def test_it_fires_nothing_else(self):
        targets = {t for row in self.client.values() for t in row["targets"]}
        for host in _hosts(self.fn):
            self.assertIn(host, targets)

    def test_the_rows_bracket_inside_a_client_window(self):
        events = [n.args[0].value for n in ast.walk(self.fn)
                  if isinstance(n, ast.Call) and getattr(n.func, "id", "") == "census_event"]
        self.assertIn("window-start", events)
        self.assertIn("window-end", events)
        windows = {kw.value.value for n in ast.walk(self.fn)
                   if isinstance(n, ast.Call) and getattr(n.func, "id", "") == "census_event"
                   for kw in n.keywords if kw.arg == "window"}
        self.assertEqual(windows, {"client"})


def _hosts(fn):
    """Every address or host name the body's literals reach: IP literals,
    URL hosts, and dotted names under a TLD-looking suffix."""
    out = set()
    for s in _strings(fn):
        if "://" in s:
            out.add(urllib.parse.urlsplit(s).hostname)
            continue
        try:
            out.add(str(ipaddress.ip_address(s)))
            continue
        except ValueError:
            pass
        if re.fullmatch(r"[a-z0-9-]+(\.[a-z0-9-]+)+", s) and not s.endswith(".jsonl"):
            out.add(s)
    return out


class Unownable(unittest.TestCase):
    TEST_NET = (ipaddress.ip_network("192.0.2.0/24"), ipaddress.ip_network("2001:db8::/32"))

    def test_every_target_is_unownable(self):
        for name in NAMES:
            for host in _hosts(_body_tree(name)):
                with self.subTest(name, host=host):
                    try:
                        ip = ipaddress.ip_address(host)
                    except ValueError:
                        self.assertTrue(host.endswith(".invalid"), host)
                        continue
                    self.assertTrue(any(ip in net for net in self.TEST_NET), host)

    def test_no_real_domain_anywhere_in_the_file(self):
        # The client's own endpoint names are extracted data; they live in
        # the gitignored path table, never here.
        real = re.compile(r"\b[a-z0-9-]+\.(com|net|org|io|is|cn)\b", re.I)
        for name in NAMES:
            with self.subTest(name):
                self.assertIsNone(real.search(_text(name)))


class Refusal(unittest.TestCase):
    def test_nothing_runs_before_the_guard(self):
        """Up to `if not census_guard(): return`, the body only defines:
        functions, imports, and assignments of literals or `os.path.join`."""
        def harmless(st):
            if isinstance(st, (ast.FunctionDef, ast.Import, ast.ImportFrom)):
                return True
            if isinstance(st, ast.Assign):
                for n in ast.walk(st.value):
                    if isinstance(n, ast.Call) and ast.unparse(n.func) != "os.path.join":
                        return False
                return True
            return False

        for name in NAMES:
            with self.subTest(name):
                fn = _body_tree(name)
                for st in fn.body:
                    if isinstance(st, ast.If) and ast.unparse(st.test) == "not census_guard()":
                        self.assertIsInstance(st.body[0], ast.Return)
                        break
                    self.assertTrue(harmless(st), ast.unparse(st)[:80])
                else:
                    self.fail("no `if not census_guard(): return`")


class Timeouts(unittest.TestCase):
    def test_the_timeout_covers_the_waits(self):
        for name in NAMES:
            with self.subTest(name):
                head = _parts(name)[0]
                timeout = int(re.search(r"(?m)^EOCI_CENSUS_TIMEOUT_S = (\d+)$", head)[1])
                waits = sum(v for k, v in _assigned(_body_tree(name)).items()
                            if k.endswith("_MS") and isinstance(v, int))
                self.assertGreater(waits, 0)
                self.assertLessEqual(waits / 1000 + LOGIN_SPARE_S, timeout)


if __name__ == "__main__":
    unittest.main()
