"""The `nodegraphs` content section: CCP's node graphs, flat, for the server's runtime.

The AIR new-player experience (#205) is node graphs: a server master graph
drives the client's graph with blackboard messages, and its scenes are
subgraphs [V `nodegraph/common/`, decompiled, read on build 3707965]. The
server runtime (`server/crates/eve-server/src/node_graph/`) reads them from
the bundle, so the graphs are the client build's own and nothing about them
is in git.

Written into the bundle (every field escapes ``% ; = | , TAB LF CR`` as
``%XX``, so a field never holds a separator):

    nodegraphs/graphs.tsv     graph_id, name, tags (comma list), parent (or -)
    nodegraphs/nodes.tsv      graph_id, node guid, node type name, atom name
                              (or -), params, atom_params, edges
    nodegraphs/nodetypes.tsv  type_id, name, in_ports, out_ports, input
                              defaults, output keys
    nodegraphs/atoms.tsv      atom_id, name, atom_type, input defaults,
                              output keys

A parameter list is ``key=VALUE;...`` with VALUE ``n:`` (None), ``i:<int>``
(bools as 0/1), ``f:<repr>``, ``s:<text>``, ``l:<python list repr>``,
``v:<x,y,z>`` or ``c:<node>.<param>`` (wired from another node's output).
Edges are ``port:guid,guid|...``. The node's ``atom_id`` parameter is written
as the atom's name in its own column, not as a parameter.

The client tables come from `fsd.rows` (``nodegraphs``, ``nodetypes``,
``atoms``); the edges (``graphNode.connections``) decode by
`fsd.CFSD_ELEMENT_KINDS` (#292). The derivation is pure; the unit tests feed
it invented graphs. `gather` is the only client reader.

A decode problem warns and never stops the bundle build: one optional
runtime reads these files, and every other section is still good. A node
whose type or atom is unknown is left out (an edge or a wire to it reads as
nothing in the runtime, `node_graph::data`), an edge to a node its graph
lacks is written and ignored by the runtime, and a client whose tables do not
decode at all gets the four files with their headers and no rows.
"""
import os
import sys

SECTION = "nodegraphs"
DEPENDS = ()
CFSD = ("nodegraphs", "nodetypes", "atoms")

_HERE = os.path.dirname(os.path.abspath(__file__))

GRAPHS_HEADER = ("graph_id", "name", "tags", "parent")
NODES_HEADER = ("graph_id", "node", "type", "atom", "params", "atom_params", "edges")
TYPES_HEADER = ("type_id", "name", "in_ports", "out_ports", "defaults", "outputs")
ATOMS_HEADER = ("atom_id", "name", "atom_type", "defaults", "outputs")

SOURCES = ["res:/staticdata/%s.fsdbinary" % s for s in CFSD]

_ESC = {c: "%%%02X" % ord(c) for c in "%;=|,\t\n\r"}


def esc(s):
    """One field with every separator `%XX`-escaped."""
    return "".join(_ESC.get(ch, ch) for ch in str(s))


def enc_value(p):
    """A parameter dict (`value` or `connection`) as one VALUE."""
    if not isinstance(p, dict):
        return "n:"
    conn = p.get("connection")
    if conn and conn.get("node"):
        return "c:%s.%s" % (esc(conn["node"]), esc(conn.get("parameter") or ""))
    v = p.get("value")
    if v is None:
        return "n:"
    if isinstance(v, bool):
        return "i:%d" % int(v)
    if isinstance(v, int):
        return "i:%d" % v
    if isinstance(v, float):
        return "f:%r" % v
    if isinstance(v, str):
        return "s:" + esc(v)
    if isinstance(v, tuple):
        return "v:" + esc(",".join(repr(x) for x in v))
    if isinstance(v, list):
        return "l:" + esc(repr(v))
    return "s:" + esc(repr(v))


def enc_params(d):
    return ";".join("%s=%s" % (esc(k), enc_value(v)) for k, v in sorted((d or {}).items()))


def _defaults(params):
    ins = (params or {}).get("inputs") or []
    return ";".join("%s=%s" % (esc(p["parameterKey"]), enc_value({"value": p.get("defaultValue")}))
                    for p in ins if p.get("parameterKey"))


def _outputs(params):
    return ",".join(esc(p["parameterKey"]) for p in ((params or {}).get("outputs") or [])
                    if p.get("parameterKey"))


def derive(graphs, nodetypes, atoms):
    """`(graph lines, node lines, type lines, atom lines, problems)`.

    Each argument is `{id: row}` as `fsd.rows` returns it. A node whose type is
    not in `nodetypes`, or an edge to a node its graph does not have, is a
    problem: the decode is not what this reads. A problem node is left out,
    and the caller warns."""
    tname = {k: v.get("name") for k, v in nodetypes.items()}
    aname = {k: v.get("name") for k, v in atoms.items()}
    g_lines, n_lines, problems = [], [], []
    for gid in sorted(graphs):
        g = graphs[gid]
        parent = g.get("parentGraph")
        g_lines.append("%d\t%s\t%s\t%s" % (gid, esc(g.get("name") or ""),
                                           ",".join(esc(t) for t in (g.get("tags") or [])),
                                           "-" if parent is None else parent))
        nodes = g.get("nodes") or {}
        for nid in sorted(nodes):
            n = nodes[nid]
            if n.get("nodeType") not in tname:
                problems.append("graph %d node %s: node type %r" % (gid, nid, n.get("nodeType")))
                continue
            params = dict(n.get("nodeParameters") or {})
            atom = "-"
            a = params.pop("atom_id", None)
            if isinstance(a, dict) and isinstance(a.get("value"), int):
                if a["value"] not in aname:
                    problems.append("graph %d node %s: atom %d" % (gid, nid, a["value"]))
                    continue
                atom = esc(aname[a["value"]])
            cn = n.get("connections") or {}
            if not isinstance(cn, dict):
                problems.append("graph %d node %s: connections %r" % (gid, nid, type(cn).__name__))
                continue
            for port, targets in cn.items():
                for t in targets:
                    if t not in nodes:
                        problems.append("graph %d node %s: edge %s to %s" % (gid, nid, port, t))
            edges = "|".join("%s:%s" % (esc(p), ",".join(esc(t) for t in ts))
                             for p, ts in sorted(cn.items()))
            n_lines.append("%d\t%s\t%s\t%s\t%s\t%s\t%s" % (
                gid, esc(nid), esc(tname[n["nodeType"]]), atom, enc_params(params),
                enc_params(n.get("atomParameters")), edges))
    t_lines = []
    for k in sorted(nodetypes):
        t = nodetypes[k]
        ports = t.get("ports") or {}
        t_lines.append("%d\t%s\t%s\t%s\t%s\t%s" % (
            k, esc(t.get("name") or ""), ",".join(esc(p) for p in (ports.get("inPorts") or [])),
            ",".join(esc(p) for p in (ports.get("outPorts") or [])),
            _defaults(t.get("parameters")), _outputs(t.get("parameters"))))
    a_lines = []
    for k in sorted(atoms):
        a = atoms[k]
        a_lines.append("%d\t%s\t%s\t%s\t%s" % (
            k, esc(a.get("name") or ""), a.get("atomType"),
            _defaults(a.get("parameters")), _outputs(a.get("parameters"))))
    return g_lines, n_lines, t_lines, a_lines, problems


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """The client's tables: `(graphs, nodetypes, atoms)`."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    c = ctx.client
    return fsd.rows(c, "nodegraphs"), fsd.rows(c, "nodetypes"), fsd.rows(c, "atoms")


def _files(ctx, tables=None):
    """The four files and the problems. Tables that do not decode are one
    problem and four header-only files, not an exception. `tables` stands in
    for `gather(ctx)` (the tests)."""
    try:
        g, n, t, a, problems = derive(*(gather(ctx) if tables is None else tables))
    except Exception as e:  # noqa: BLE001 -- a decode failure only warns
        g, n, t, a = [], [], [], []
        problems = ["the client tables did not decode (%s: %s), so the section is empty"
                    % (type(e).__name__, e)]
    return [("nodegraphs/graphs.tsv", GRAPHS_HEADER, g, SOURCES),
            ("nodegraphs/nodes.tsv", NODES_HEADER, n, SOURCES),
            ("nodegraphs/nodetypes.tsv", TYPES_HEADER, t, SOURCES),
            ("nodegraphs/atoms.tsv", ATOMS_HEADER, a, SOURCES)], problems


def warn(ctx, problems):
    """Each problem as a warning: the first 20, then how many more."""
    for p in problems[:20]:
        ctx.note("nodegraphs: WARNING " + p)
    if len(problems) > 20:
        ctx.note("nodegraphs: WARNING and %d more" % (len(problems) - 20))


def section(ctx):
    files, problems = _files(ctx)
    warn(ctx, problems)
    ctx.note("nodegraphs: %d graphs, %d nodes, %d node types, %d atoms"
             % tuple(len(f[2]) for f in files))
    return files


def check(ctx):
    """The files against a fresh derive. A decode problem warns here as it
    does in `section`; it is not a check failure."""
    content = _content()
    files, problems = _files(ctx)
    warn(ctx, problems)
    bad = []
    for rel, _, want, _ in files:
        got = ["\t".join(r) for r in content.read_rows(ctx.path(rel))]
        if got != want:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)" % (rel, len(got), len(want)))
    return bad
