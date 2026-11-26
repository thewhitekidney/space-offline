"""Unit tests for `tools/content_nodegraphs.py`.

Invented graphs only (ids in the 99xxxx band). The bundle-backed run is the
`nodegraphs` section of `tools/content.py` (`tools/up.sh`).
Run: `python3 -I tools/test_content_nodegraphs.py -q`.
"""

from __future__ import annotations

import os
import sys
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content_nodegraphs as N  # noqa: E402

G, SUB = 990001, 990002
T_ROOT, T_DELAY, T_ATOM = 990101, 990102, 990103
A_SAY = 990201

TYPES = {
    T_ROOT: {"name": "Root", "ports": {"outPorts": ["output"]}},
    T_DELAY: {"name": "Delay", "ports": {"inPorts": ["input"], "outPorts": ["output"]},
              "parameters": {"inputs": [{"parameterKey": "delay", "defaultValue": 1.0}]}},
    T_ATOM: {"name": "Action", "ports": {"inPorts": ["input"], "outPorts": ["output"]}},
}
ATOMS = {
    A_SAY: {"name": "Say; loudly", "atomType": 1,
            "parameters": {"inputs": [{"parameterKey": "text", "defaultValue": "hi"},
                                      {"parameterKey": "loud", "defaultValue": True}],
                           "outputs": [{"parameterKey": "said"}]}},
}


def _graphs():
    return {
        G: {"name": "A graph", "tags": ["server", "x,y"], "nodes": {
            "r": {"nodeType": T_ROOT, "connections": {"output": ["d"]}},
            "d": {"nodeType": T_DELAY, "nodeParameters": {"delay": {"value": 2.5}},
                  "connections": {"output": ["s"]}},
            "s": {"nodeType": T_ATOM,
                  "nodeParameters": {"atom_id": {"value": A_SAY}},
                  "atomParameters": {"text": {"connection": {"node": "d", "parameter": "out"}},
                                     "where": {"value": (1.0, 2.0, 3.0)},
                                     "ids": {"value": [1, 2]},
                                     "none": {"value": None}}},
        }},
        SUB: {"name": "Sub", "parentGraph": G, "nodes": {}},
    }


class DeriveTest(unittest.TestCase):
    def test_graph_rows_carry_escaped_tags_and_the_parent(self):
        g, _, _, _, problems = N.derive(_graphs(), TYPES, ATOMS)
        self.assertEqual(problems, [])
        self.assertEqual(g, ["%d\tA graph\tserver,x%%2Cy\t-" % G, "%d\tSub\t\t%d" % (SUB, G)])

    def test_node_rows_name_the_type_and_atom_and_encode_every_value_kind(self):
        _, n, _, _, _ = N.derive(_graphs(), TYPES, ATOMS)
        by = {line.split("\t")[1]: line.split("\t") for line in n}
        self.assertEqual(sorted(by), ["d", "r", "s"])
        self.assertEqual(by["r"][2:4], ["Root", "-"])
        self.assertEqual(by["r"][6], "output:d")
        self.assertEqual(by["d"][4], "delay=f:2.5")
        s = by["s"]
        self.assertEqual(s[2:5], ["Action", "Say%3B loudly", ""])
        self.assertEqual(s[5], "ids=l:[1%2C 2];none=n:;text=c:d.out;where=v:1.0%2C2.0%2C3.0")

    def test_types_and_atoms_keep_ports_defaults_and_outputs(self):
        _, _, t, a, _ = N.derive(_graphs(), TYPES, ATOMS)
        self.assertEqual(t[1], "%d\tDelay\tinput\toutput\tdelay=f:1.0\t" % T_DELAY)
        self.assertEqual(a, ["%d\tSay%%3B loudly\t1\ttext=s:hi;loud=i:1\tsaid" % A_SAY])

    def test_an_unknown_type_atom_or_dangling_edge_is_a_problem(self):
        graphs = _graphs()
        nodes = graphs[G]["nodes"]
        nodes["x"] = {"nodeType": 990199}
        nodes["y"] = {"nodeType": T_ATOM, "nodeParameters": {"atom_id": {"value": 990299}}}
        nodes["d"]["connections"]["output"].append("gone")
        _, n, _, _, problems = N.derive(graphs, TYPES, ATOMS)
        self.assertEqual(len(problems), 3)
        self.assertNotIn("x", [line.split("\t")[1] for line in n])
        self.assertNotIn("y", [line.split("\t")[1] for line in n])

    def test_a_problem_warns_and_the_good_nodes_are_still_written(self):
        graphs = _graphs()
        graphs[G]["nodes"]["x"] = {"nodeType": 990199}
        notes = []
        ctx = type("Ctx", (), {"note": lambda self, m: notes.append(m)})()
        files, problems = N._files(ctx, (graphs, TYPES, ATOMS))  # noqa: SLF001
        N.warn(ctx, problems)
        self.assertEqual(len(problems), 1)
        self.assertTrue(notes[0].startswith("nodegraphs: WARNING graph %d node x" % G), notes)
        nodes = {f[0]: f[2] for f in files}["nodegraphs/nodes.tsv"]
        self.assertEqual(sorted(line.split("\t")[1] for line in nodes), ["d", "r", "s"])

    def test_tables_that_do_not_decode_give_empty_files_and_one_warning(self):
        ctx = type("Ctx", (), {"note": lambda self, m: None})()
        bad = {G: {"nodes": ["not", "a", "dict"]}}
        files, problems = N._files(ctx, (bad, TYPES, ATOMS))  # noqa: SLF001
        self.assertEqual(len(problems), 1)
        self.assertIn("did not decode", problems[0])
        self.assertEqual([f[2] for f in files], [[], [], [], []])

    def test_many_problems_warn_twenty_and_a_count(self):
        notes = []
        ctx = type("Ctx", (), {"note": lambda self, m: notes.append(m)})()
        N.warn(ctx, ["p%d" % i for i in range(25)])
        self.assertEqual(len(notes), 21)
        self.assertEqual(notes[-1], "nodegraphs: WARNING and 5 more")

    def test_no_field_holds_a_separator(self):
        graphs = _graphs()
        graphs[G]["nodes"]["d"]["nodeParameters"]["delay"] = {"value": "a\tb\nc|d=e;f%"}
        _, n, _, _, _ = N.derive(graphs, TYPES, ATOMS)
        for line in n:
            self.assertEqual(len(line.split("\t")), 7)
            self.assertNotIn("\n", line)


if __name__ == "__main__":
    unittest.main()
