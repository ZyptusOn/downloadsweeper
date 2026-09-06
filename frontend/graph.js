const {
  createElement: h,
  useState,
  useMemo,
  useEffect,
  useCallback,
  useRef,
  memo,
} = React;
const {
  ReactFlow,
  ReactFlowProvider,
  Handle,
  Position,
  Controls,
  MiniMap,
  Background,
  applyNodeChanges,
  useUpdateNodeInternals,
} = window.ReactFlow;
export const sizeLabel = (n) =>
  n >= 1073741824
    ? (n / 1073741824).toFixed(2) + " GB"
    : n >= 1048576
      ? (n / 1048576).toFixed(1) + " MB"
      : n >= 1024
        ? (n / 1024).toFixed(1) + " KB"
        : n + " B";
export const classLabel = {
  normal: "可拆散",
  atomic: "整体保护",
  container: "复用容器",
};
const colors = [
  "#238878",
  "#657dc2",
  "#bc8850",
  "#9b6bac",
  "#4e8fa9",
  "#899850",
];
const field = (label, control) =>
  h("label", { className: "node-field" }, h("span", null, label), control);
function FolderIcon() {
  return h(
    "svg",
    {
      width: 17,
      height: 17,
      viewBox: "0 0 24 24",
      fill: "none",
      stroke: "currentColor",
      strokeWidth: 1.7,
    },
    h("path", { d: "M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z" }),
  );
}
const DirectoryNode = memo(function ({ id, data, selected }) {
  const updateInternals = useUpdateNodeInternals();
  const n = data.node,
    edit = data.editable,
    root = id === "root",
    type = data.type;
  const folded = data.folded;
  useEffect(() => {
    updateInternals(id);
  }, [id, folded, data.collapsed, data.childCount, updateInternals]);
  const [name, setName] = useState(n.name),
    [ext, setExt] = useState((n.extensions || []).join(", ")),
    [note, setNote] = useState(n.note || "");
  useEffect(() => {
    setName(n.name);
    setExt((n.extensions || []).join(", "));
    setNote(n.note || "");
  }, [n.name, JSON.stringify(n.extensions), n.note]);
  const change = (patch) => data.update?.(id, patch);
  const files = data.files || [];
  return h(
    "div",
    {
      className:
        "folder-node " +
        (root ? "root-node " : "") +
        (data.orphan ? "orphan " : "") +
        (folded ? "folded " : "") +
        (selected ? "selected" : ""),
      style: { "--node-color": data.color },
    },
    !root &&
      h(Handle, {
        type: "target",
        position: Position.Top,
        id: "parent",
        isConnectable: edit,
        className: "port",
      }),
    h(Handle, {
      type: "source",
      position: Position.Bottom,
      id: "children",
      isConnectable: edit,
      className: "port",
    }),
    h(
      "header",
      { className: "node-title" },
      h(FolderIcon),
      h("strong", { title: n.name }, n.name),
      data.orphan
        ? h("span", { className: "node-label warning" }, "未连接")
        : h(
            "span",
            { className: "node-label" },
            root
              ? "根目录"
              : type === "target"
                ? n.parent === "root"
                  ? "一级分类"
                  : "子目录"
                : data.delta || "文件夹",
          ),
      (!root || type === "target") &&
        h(
          "button",
          {
            type: "button",
            className: "node-fold-toggle nodrag nopan",
            "aria-label": `${folded ? "展开" : "收起"}「${n.name}」参数`,
            "aria-expanded": !folded,
            title: folded ? "展开节点参数" : "收起节点参数",
            onClick: (e) => {
              e.stopPropagation();
              data.toggleDetails(id);
            },
          },
          h("span", { "aria-hidden": true }, folded ? "▸" : "▾"),
          "参数",
        ),
    ),
    folded &&
      (type === "target" || !root) &&
      h(
        "div",
        { className: "node-fold-summary nodrag" },
        h(
          "span",
          null,
          type === "target"
            ? root
              ? "整理根目录"
              : n.rule_type === "complex"
                ? "语义分类"
                : "扩展名分类"
            : "目录类型",
        ),
        type !== "target" &&
          !root &&
          h(
            "span",
            { className: "folded-class " + n.class },
            classLabel[n.class] || "可拆散",
          ),
      ),
    !folded &&
      type === "target" &&
      !root &&
      h(
        "div",
        { className: "node-params nodrag nowheel" },
        field(
          "文件夹名称",
          h("input", {
            value: name,
            disabled: !edit,
            "aria-label": `${n.name}文件夹名称`,
            onChange: (e) => setName(e.target.value),
            onBlur: () => {
              if (name !== n.name) change({ name });
            },
            onKeyDown: (e) => {
              if (e.key === "Enter") e.target.blur();
            },
          }),
        ),
        field(
          "分类规则",
          h(
            "select",
            {
              value: n.rule_type,
              disabled: !edit || n.parent === "root",
              onChange: (e) => change({ rule_type: e.target.value }),
            },
            h("option", { value: "simple" }, "扩展名 · 本地规则"),
            h("option", { value: "complex" }, "语义 · AI 理解"),
          ),
        ),
        n.rule_type === "simple" &&
          field(
            "扩展名",
            h("input", {
              value: ext,
              disabled: !edit,
              placeholder: "pdf, docx, txt",
              "aria-label": `${n.name}扩展名`,
              onChange: (e) => setExt(e.target.value),
              onBlur: () => {
                const extensions = [
                  ...new Set(
                    ext
                      .split(/[,，\s]+/)
                      .map((s) => s.trim().replace(/^\./, "").toLowerCase())
                      .filter(Boolean),
                  ),
                ];
                if (JSON.stringify(extensions) !== JSON.stringify(n.extensions))
                  change({ extensions });
              },
            }),
          ),
        field(
          "分类备注",
          h("textarea", {
            value: note,
            disabled: !edit,
            rows: 2,
            placeholder: "说明这里应该放什么…",
            onChange: (e) => setNote(e.target.value),
            onBlur: () => {
              if (note !== n.note) change({ note });
            },
          }),
        ),
        field(
          "文件示例",
          h(
            "div",
            { className: "node-examples" },
            ...(n.examples || []).map((path) =>
              h(
                "span",
                { key: path, className: "example-tag", title: path },
                path.split("/").pop(),
                edit &&
                  h(
                    "button",
                    {
                      title: "移除示例",
                      onClick: () =>
                        change({
                          examples: n.examples.filter((p) => p !== path),
                        }),
                    },
                    "×",
                  ),
              ),
            ),
            edit &&
              h(
                "button",
                { className: "text-btn", onClick: () => data.pickExamples(id) },
                "+ 选择具体文件",
              ),
          ),
        ),
        n.parent === "root" &&
          field(
            "复用现有目录",
            h(
              "select",
              {
                value: n.mapping || "",
                disabled: !edit,
                onChange: (e) => change({ mapping: e.target.value || null }),
              },
              h("option", { value: "" }, "新建 / 使用同名目录"),
              ...(data.containers || []).map((e) =>
                h("option", { key: e.id, value: e.id }, e.name),
              ),
            ),
          ),
        n.parent === "root" &&
          h(
            "small",
            { className: "node-help" },
            n.extensions.includes("@folder")
              ? "整体保护目录专用类别"
              : n.extensions.includes("*")
                ? "接收其他类别未匹配的格式"
                : "一级按格式分流，子级可使用 AI 分类",
          ),
      ),
    !folded &&
      !root &&
      type !== "target" &&
      h(
        "div",
        { className: "node-params nodrag nowheel" },
        !root &&
          field(
            "目录处理方式",
            data.canClass
              ? h(
                  "select",
                  {
                    "aria-label": `${n.name}目录类型`,
                    value: n.class,
                    onChange: (e) => data.setClass(n.id, e.target.value),
                  },
                  ...Object.entries(classLabel).filter(([v]) => !data.unscanned || v !== "normal").map(([v, l]) =>
                    h("option", { key: v, value: v }, l),
                  ),
                )
              : h(
                  "span",
                  { className: "class-chip " + n.class },
                  classLabel[n.class] || "可拆散",
                ),
          ),
        n.class === "atomic" &&
          h("small", { className: "node-help" }, "内部文件与目录作为一个整体"),
        data.unscanned && n.class === "container" &&
          h("small", { className: "node-help" }, "可在目标树一级分类中选择复用；仅接收新文件，已有内容保持不动"),
      ),
    type !== "target" &&
      h(
        "div",
        { className: "node-files nodrag nowheel" },
        h(
          "div",
          { className: "node-files-head" },
          h("span", null, data.unscanned ? "内部未扫描" : `${files.length} 个直接文件`),
          !data.unscanned && h("span", null, sizeLabel(files.reduce((s, f) => s + f.size, 0))),
        ),
        h(
          "div",
          { className: "node-file-list nowheel", tabIndex: 0 },
          files.length
            ? files.map((f, i) =>
                h(
                  React.Fragment,
                  { key: f.id },
                  data.group &&
                    (!i || files[i - 1].extension !== f.extension) &&
                    h(
                      "div",
                      { className: "file-group" },
                      f.extension ? f.extension.toUpperCase() : "无扩展名",
                    ),
                  h(
                    "div",
                    { className: "node-file", title: f.id },
                    h("span", { className: "file-dot" }),
                    h("span", null, f.name),
                    h("small", null, sizeLabel(f.size)),
                  ),
                ),
              )
            : h("div", { className: "node-empty" }, data.unscanned ? (n.class === "container" ? "复用容器保留原位" : "可按权限识别并整体移动，不拆散") : "此目录没有散装文件"),
        ),
      ),
    !folded &&
      root &&
      type === "target" &&
      h(
        "div",
        { className: "root-description" },
        "按格式建立一级目录",
        h("small", null, "从下方端口连接你的分类"),
      ),
    h("footer", {className:"node-quick-actions nodrag nopan"},
      type === "target" && h("button", {
        type:"button", className:"node-quick-button", disabled:!edit,
        "aria-label":`在「${n.name}」下新建子目录`, title:edit ? "新建子目录" : "当前视图只读",
        onClick:e=>{e.stopPropagation();data.addChild?.(id);},
      }, h("span", {"aria-hidden":true}, "+")),
      h("button", {
        type:"button", className:"node-quick-button node-branch-toggle", disabled:!data.childCount,
        "aria-label":`${data.collapsed ? "展开" : "收起"}「${n.name}」子目录`,
        "aria-expanded":data.childCount ? !data.collapsed : undefined,
        title:data.childCount ? `${data.collapsed ? "展开" : "收起"}子目录（${data.childCount}）` : "没有子目录",
        onClick:e=>{e.stopPropagation();data.toggle(id);},
      }, h("svg", {width:16,height:16,viewBox:"0 0 20 20",fill:"none",stroke:"currentColor",strokeWidth:1.8,"aria-hidden":true},
        h("path", {d:data.collapsed ? "M5 7l5 5 5-5" : "M5 13l5-5 5 5"}))),
    ),
  );
});
const nodeTypes = { directory: DirectoryNode };
export function isActive(nodes, id) {
  const seen = new Set();
  while (id !== "root") {
    if (seen.has(id)) return false;
    seen.add(id);
    const node = nodes.find((n) => n.id === id);
    if (!node?.parent) return false;
    id = node.parent;
  }
  return true;
}
export function validConnection(nodes, source, target) {
  if (!source || !target || target === "root" || source === target)
    return false;
  let p = source;
  const seen = new Set();
  while (p && p !== "root") {
    if (p === target || seen.has(p)) return false;
    seen.add(p);
    p = nodes.find((n) => n.id === p)?.parent;
  }
  return true;
}
function height(node, type, folded, fileCounts) {
  if (type === "target")
    return folded.has(node.id)
      ? 138
      : node.id === "root"
        ? 180
        : node.parent === "root"
          ? 650
          : 550;
  const count = fileCounts.get(node.id) || 0;
  const files = count ? Math.min(220, count * 40) : 55;
  return (
    145 + files + (node.id === "root" ? 0 : folded.has(node.id) ? 48 : 145)
  );
}
export function layout(
  nodes,
  type,
  folded = new Set(),
  fileCounts = new Map(),
  dimensions = new Map(),
) {
  const positions = new Map(),
    byParent = new Map();
  const root = { id: "root", parent: null };
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const nodeHeight = (n) => dimensions.get(n.id)?.height || height(n, type, folded, fileCounts);
  const nodeWidth = (n) => dimensions.get(n.id)?.width || 360;
  for (const n of nodes.filter((n) => n.id !== "root")) {
    const p = n.parent || "orphans";
    if (!byParent.has(p)) byParent.set(p, []);
    byParent.get(p).push(n);
  }
  const sizes = new Map();
  const columnGap = 24, rowGap = 60;
  function measure(n, seen = new Set()) {
    if (seen.has(n.id)) return {width:nodeWidth(n), height:nodeHeight(n)};
    seen.add(n.id);
    const children = byParent.get(n.id) || [];
    const parts = children.map(c => measure(c, new Set(seen)));
    const size = {
      width:Math.max(nodeWidth(n), parts.reduce((sum,c)=>sum+c.width,0) + Math.max(0,parts.length-1)*columnGap),
      height:nodeHeight(n) + (parts.length ? rowGap + Math.max(...parts.map(c=>c.height)) : 0),
    };
    sizes.set(n.id,size);
    return size;
  }
  function place(n, left, top) {
    if (positions.has(n.id)) return;
    const size = sizes.get(n.id) || {width:nodeWidth(n),height:nodeHeight(n)};
    positions.set(n.id,{x:left+(size.width-nodeWidth(n))/2,y:top});
    const children = byParent.get(n.id) || [];
    const span = children.reduce((sum,c)=>sum+sizes.get(c.id).width,0) + Math.max(0,children.length-1)*columnGap;
    let x=left+(size.width-span)/2;
    for (const child of children) {
      place(child,x,top+nodeHeight(n)+rowGap);
      x+=sizes.get(child.id).width+columnGap;
    }
  }
  measure(root);
  // Parent above, children in a compact row; each component reserves its full bounds.
  place(root,-sizes.get("root").width/2,0);
  let oy=sizes.get("root").height+100;
  const detached = nodes.filter(n=>n.id!=="root" && (!n.parent || (n.parent!=="root" && !byId.has(n.parent))));
  for (const n of [...detached,...nodes]) {
    if (positions.has(n.id)) continue;
    const size=measure(n);
    place(n,-size.width/2,oy);
    oy+=size.height+60;
  }
  // Only detached component roots have a user-controlled anchor. Connected
  // branches always use the mind-map layout; translate descendants as a unit.
  if (type === "target") for (const n of detached) {
    if (!validPosition(n.position)) continue;
    const origin = positions.get(n.id);
    const dx = n.position[0] - origin.x, dy = n.position[1] - origin.y;
    for (const id of subtreeIds(nodes, n.id)) {
      const point = positions.get(id);
      if (point) positions.set(id, {x: point.x + dx, y: point.y + dy});
    }
  }
  return positions;
}
function validPosition(position) {
  return Array.isArray(position) && position.length === 2 && position.every((v) => Number.isFinite(v) && Math.abs(v) < 1e7);
}
export function subtreeIds(nodes, root) {
  const children = new Map();
  for (const node of nodes) {
    if (!children.has(node.parent)) children.set(node.parent, []);
    children.get(node.parent).push(node.id);
  }
  const found = new Set(), pending = [root];
  while (pending.length) {
    const id = pending.pop();
    if (found.has(id)) continue;
    found.add(id);
    pending.push(...(children.get(id) || []));
  }
  return found;
}
export function dragDetached(nodes, model, changes) {
  let moved = nodes;
  for (const change of changes) {
    if (change.type !== "position" || !change.position || !validPosition([change.position.x, change.position.y])) continue;
    const root = model.find((n) => n.id === change.id);
    if (!root || root.parent || root.id === "root") continue;
    const old = moved.find((n) => n.id === root.id);
    if (!old) continue;
    const dx = change.position.x - old.position.x, dy = change.position.y - old.position.y;
    const ids = subtreeIds(model, root.id);
    moved = moved.map((n) => ids.has(n.id) ? {...n, position: {x: n.position.x + dx, y: n.position.y + dy}} : n);
  }
  return moved;
}
export function projectedEntries(task, selected) {
  const ops = task.operations.filter((o) => selected.has(o.id));
  const entries = task.entries.map((e) => ({ ...e }));
  for (const e of entries) {
    const op = ops.find(
      (o) =>
        o.source === e.id ||
        (o.kind === "directory" && e.id.startsWith(o.source + "/")),
    );
    if (op) {
      e.id = op.destination + e.id.slice(op.source.length);
      e.name = e.id.split("/").pop();
      e.parent = e.id.split("/").slice(0, -1).join("/");
      e.delta = "迁入";
    }
  }
  const ids = new Set(entries.map((e) => e.id));
  for (const e of [...entries]) {
    let p = e.parent;
    while (p) {
      if (!ids.has(p)) {
        ids.add(p);
        entries.push({
          id: p,
          parent: p.split("/").slice(0, -1).join("/"),
          name: p.split("/").pop(),
          kind: "directory",
          size: 0,
          class: task.mode === "desktop" ? "atomic" : "normal",
          delta: "新目录",
        });
      }
      p = p.split("/").slice(0, -1).join("/");
    }
  }
  return entries;
}
export function visibleEntries(task, entries = task.entries) {
  return task.mode === "desktop" ? entries.filter(e => e.kind === "directory" || !["url", "lnk"].includes((e.extension || "").toLowerCase())) : entries;
}
export function Graph(props) {
  const view = `${props.task.id}:${props.type || "actual"}:${props.entries ? "preview" : "source"}`;
  return h(
    ReactFlowProvider,
    { key: view },
    h(GraphInner, { ...props, viewKey: `ds-graph-view-v3:${view}` }),
  );
}
function savedFoldSet(viewKey, name, fallback = new Set()) {
  try {
    const values = JSON.parse(sessionStorage.getItem(viewKey) || "{}")[name];
    return Array.isArray(values)
      ? new Set(values.filter((v) => typeof v === "string"))
      : fallback;
  } catch {
    return fallback;
  }
}
export function defaultCollapsed(dirs) {
  const children = new Map(),
    byId = new Map(dirs.map((n) => [n.id, n]));
  for (const n of dirs)
    children.set(n.parent, (children.get(n.parent) || 0) + 1);
  return new Set(
    dirs
      .filter((n) => {
        if (!children.has(n.id)) return false;
        let depth = 1,
          parent = n.parent,
          seen = new Set([n.id]);
        while (parent && parent !== "root" && !seen.has(parent)) {
          seen.add(parent);
          depth++;
          parent = byId.get(parent)?.parent;
        }
        return n.class === "atomic" || children.get(n.id) >= 5 || depth >= 3;
      })
      .map((n) => n.id),
  );
}
export function branchState(dirs, overrides) {
  const result = defaultCollapsed(dirs);
  for (const [id, closed] of overrides) {
    if (closed) result.add(id);
    else result.delete(id);
  }
  return result;
}
function savedBranches(viewKey) {
  try {
    return new Map(Object.entries(JSON.parse(sessionStorage.getItem(viewKey) || "{}").branchOverrides || {})
      .filter(([id, closed]) => typeof id === "string" && typeof closed === "boolean"));
  } catch {
    return new Map();
  }
}
function GraphInner({
  task,
  type = "actual",
  editable = false,
  entries: givenEntries,
  onNodes,
  onClass,
  onExamples,
  compact = false,
  viewKey,
}) {
  const model = task.nodes || [],
    actual = useMemo(() => visibleEntries(task, givenEntries || task.entries), [task.mode, task.entries, givenEntries]);
  const dirs = useMemo(
    () =>
      type === "target"
        ? model
        : actual
            .filter((e) => e.kind === "directory")
            .map((e) => ({ ...e, parent: e.parent || "root" })),
    [model, actual, type],
  );
  const [sort, setSort] = useState("name"),
    [group, setGroup] = useState(false),
    [branchOverrides, setBranchOverrides] = useState(() => savedBranches(viewKey)),
    [expandedParams, setExpandedParams] = useState(() =>
      savedFoldSet(viewKey, "parametersOpen"),
    ),
    [selectedEdges, setSelectedEdges] = useState(new Set()),
    [menu, setMenu] = useState(null),
    [flow, setFlow] = useState(null),
    [visual, setVisual] = useState([]),
    [layoutKey, setLayoutKey] = useState(0),
    [dimensions, setDimensions] = useState(new Map()),
    [full, setFull] = useState(false);
  const collapsed = useMemo(() => branchState(dirs, branchOverrides), [dirs, branchOverrides]);
  useEffect(() => {
    try {
      sessionStorage.setItem(
        viewKey,
        JSON.stringify({
          branchOverrides: Object.fromEntries(branchOverrides),
          parametersOpen: [...expandedParams],
        }),
      );
    } catch {}
  }, [viewKey, branchOverrides, expandedParams]);
  const container = useRef(null);
  const folded = new Set(
    ["root", ...dirs.map((n) => n.id)].filter((id) => !expandedParams.has(id)),
  );
  const current = useRef(model);
  current.current = model;
  const canClass = !!onClass && [1, 2].includes(task.phase);
  const update = useCallback(
    (id, patch) =>
      onNodes?.(
        current.current.map((n) => (n.id === id ? { ...n, ...patch } : n)),
      ),
    [onNodes],
  );
  const toggle = (id) =>
    setBranchOverrides((s) => {
      const next = new Map(s);
      next.set(id, !collapsed.has(id));
      return next;
    });
  const toggleDetails = (id) =>
    setExpandedParams((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const hide = (id) => {
    let node = dirs.find((n) => n.id === id);
    const seen = new Set();
    while (node?.parent) {
      if (collapsed.has(node.parent)) return true;
      if (seen.has(node.id)) return true;
      seen.add(node.id);
      node = dirs.find((n) => n.id === node.parent);
    }
    return false;
  };
  const visible = dirs.filter((n) => !hide(n.id));
  const key =
    JSON.stringify(
      visible.map((n) => [
        n.id,
        n.parent,
        n.name,
        n.rule_type,
        n.extensions,
        n.note,
        n.examples,
        n.class,
        n.mapping,
        n.position,
      ]),
    ) +
    sort +
    group +
    [...collapsed] +
    JSON.stringify([...folded]) +
    layoutKey +
    JSON.stringify([...dimensions]) +
    JSON.stringify(actual.map((e) => [e.id, e.size])) +
    editable +
    canClass;
  useEffect(() => {
    const fileCounts = new Map();
    for (const e of actual)
      if (e.kind === "file")
        fileCounts.set(
          e.parent || "root",
          (fileCounts.get(e.parent || "root") || 0) + 1,
        );
    const positioned = layout(visible, type, folded, fileCounts, dimensions),
      old = new Map(visual.map((n) => [n.id, n]));
    const roots = visible.filter((n) => n.parent === "root");
    const rootName =
      String(task.root)
        .replaceAll("\\", "/")
        .split("/")
        .filter(Boolean)
        .pop() || "Downloads";
    const all = [{ id: "root", name: rootName, parent: null }, ...visible];
    const fileMap = new Map();
    for (const e of actual.filter((e) => e.kind === "file")) {
      const parent = e.parent || "root";
      if (!fileMap.has(parent)) fileMap.set(parent, []);
      fileMap.get(parent).push(e);
    }
    const result = all.map((n) => {
      let files = [...(fileMap.get(n.id) || [])];
      files.sort((a, b) =>
        group
          ? a.extension.localeCompare(b.extension) ||
            a.name.localeCompare(b.name, "zh-CN")
          : sort === "size"
            ? b.size - a.size
            : sort === "time"
              ? b.modified_ms - a.modified_ms
              : sort === "format"
                ? a.extension.localeCompare(b.extension) ||
                  a.name.localeCompare(b.name, "zh-CN")
                : a.name.localeCompare(b.name, "zh-CN"),
      );
      let ancestor = n;
      const seen = new Set();
      while (
        ancestor.parent &&
        ancestor.parent !== "root" &&
        !seen.has(ancestor.id)
      ) {
        seen.add(ancestor.id);
        ancestor = visible.find((x) => x.id === ancestor.parent) || { id: "" };
      }
      const color =
        colors[
          Math.max(
            0,
            roots.findIndex((r) => r.id === ancestor.id),
          ) % colors.length
        ];
      return {
        id: n.id,
        type: "directory",
        position: positioned.get(n.id) ||
          old.get(n.id)?.position || { x: 0, y: 0 },
        selected: old.get(n.id)?.selected || false,
        selectable: true,
        deletable: editable && n.id !== "root",
        draggable: editable && type === "target" && n.id !== "root" && !n.parent,
        className: editable && type === "target" && n.id !== "root" && !n.parent ? "detached-draggable" : "",
        dragHandle: ".node-title",
        data: {
          node: n,
          unscanned: task.mode === "desktop" && task.entries.some((e) => e.id === n.id && e.kind === "directory"),
          type,
          editable,
          orphan:
            n.id !== "root" && type === "target" && !isActive(model, n.id),
          color,
          files,
          group,
          canClass,
          setClass: onClass,
          update,
          pickExamples: onExamples,
          containers: task.entries.filter(
            (e) =>
              e.kind === "directory" && !e.parent && e.class === "container",
          ),
          collapsed: collapsed.has(n.id),
          folded: folded.has(n.id),
          toggleDetails,
          childCount: dirs.filter((d) => d.parent === n.id).length,
          toggle,
          addChild: (parent) => add(parent),
          delta: n.delta,
        },
      };
    });
    setVisual(result);
  }, [key]);
  const edges = visible
    .filter((n) => n.parent)
    .map((n) => ({
      id: `${n.parent}->${n.id}`,
      source: n.parent,
      target: n.id,
      sourceHandle: "children",
      targetHandle: "parent",
      type: "default",
      pathOptions: { curvature: 0.42 },
      deletable: editable,
      selectable: editable,
      selected: selectedEdges.has(`${n.parent}->${n.id}`),
      style: {
        stroke:
          type === "target" && !isActive(model, n.id) ? "#c2c8cc" : visual.find((v) => v.id === n.id)?.data.color || "#87a79e",
        strokeWidth: n.parent === "root" ? 2.2 : 1.6,
        strokeOpacity: 0.65,
        strokeLinecap: "round",
      },
    }));
  const reconnect = (connection) => {
    if (!validConnection(model, connection.source, connection.target)) return;
    update(connection.target, {
      parent: connection.source,
      ...(connection.source === "root" ? { rule_type: "simple" } : {}),
      ...(connection.source !== "root" ? { mapping: null } : {}),
    });
  };
  const selectNode = (id) => {
    setVisual((nodes) => nodes.map((n) => ({ ...n, selected: n.id === id })));
    setSelectedEdges(new Set());
    container.current?.focus({ preventScroll: true });
  };
  const selectedNodeIds = new Set(
    visual.filter((n) => n.selected && n.id !== "root").map((n) => n.id),
  );
  const deleteSelection = (nodeIds = selectedNodeIds) => {
    if (!editable || (!nodeIds.size && !selectedEdges.size)) return;
    // One atomic tree edit: deleting a node also detaches surviving children.
    // Built-in node/edge deletion callbacks used to race and overwrite each other.
    const detached = new Set(
      edges.filter((e) => selectedEdges.has(e.id)).map((e) => e.target),
    );
    const nodes = current.current
      .filter((n) => !nodeIds.has(n.id))
      .map((n) =>
        nodeIds.has(n.parent) || detached.has(n.id)
          ? { ...n, parent: null, mapping: null, position: (() => { const p = visual.find((v) => v.id === n.id)?.position; return p ? [p.x, p.y] : null; })() }
          : n,
      );
    onNodes?.(nodes);
    setSelectedEdges(new Set());
    setMenu(null);
  };
  const add = (parent = null) => {
    if (!editable || type !== "target") return;
    const id = crypto.randomUUID();
    let name = "新建文件夹",
      i = 1;
    while (model.some((n) => n.name === name)) name = `新建文件夹 ${i++}`;
    const bounds = container.current?.querySelector(".graph-canvas")?.getBoundingClientRect();
    const point = flow && bounds && menu ? flow.project({x: menu.x - bounds.left, y: menu.y - bounds.top}) : null;
    onNodes([
      ...model,
      {
        id,
        parent,
        name,
        rule_type: parent && parent !== "root" ? "complex" : "simple",
        extensions: [],
        note: "",
        examples: [],
        mapping: null,
        position: !parent && point ? [point.x, point.y] : null,
      },
    ]);
    if (parent) setBranchOverrides(previous => new Map(previous).set(parent,false));
    setExpandedParams(previous => new Set(previous).add(id));
    setMenu(null);
  };
  const fitPending = useRef(true);
  const fitOverview = useCallback(() => {
    const canvas = container.current?.querySelector(".graph-canvas");
    if (!flow || !canvas || !visual.length) return;
    let left=Infinity, right=-Infinity, top=Infinity, bottom=-Infinity;
    for (const node of visual) {
      const size=dimensions.get(node.id) || {width:360,height:180};
      left=Math.min(left,node.position.x); right=Math.max(right,node.position.x+size.width);
      top=Math.min(top,node.position.y); bottom=Math.max(bottom,node.position.y+size.height);
    }
    const zoom=Math.max(0.12,Math.min(1,(canvas.clientWidth-64)/(right-left),(canvas.clientHeight-64)/(bottom-top)));
    flow.setViewport({x:(canvas.clientWidth-(right-left)*zoom)/2-left*zoom,y:(canvas.clientHeight-(bottom-top)*zoom)/2-top*zoom,zoom},{duration:250});
  }, [flow, visual, dimensions]);
  useEffect(() => { fitPending.current = true; }, [flow, layoutKey, full, visible.map(n=>n.id).join("|")]);
  useEffect(() => {
    if (!fitPending.current || !flow || !visual.length) return;
    const timer = setTimeout(() => {
      fitOverview();
      fitPending.current = false;
    }, 100);
    return () => clearTimeout(timer);
  }, [fitOverview, layoutKey, full]);
  return h(
    "div",
    {
      className:
        "graph-wrapper " +
        (compact ? "compact" : "") +
        (full ? " fullscreen" : ""),
      ref: container,
      tabIndex: 0,
      "aria-label": type === "target" ? "目标目录节点画布" : "实际目录节点画布",
      onKeyDown: (e) => {
        if (
          !editable ||
          !["Delete", "Backspace"].includes(e.key) ||
          e.target.closest('input,textarea,select,[contenteditable="true"]')
        )
          return;
        e.preventDefault();
        e.stopPropagation();
        deleteSelection();
      },
    },
    h(
      "div",
      { className: "graph-toolbar" },
      h(
        "div",
        { className: "graph-legend" },
        h("span", { className: "legend-dot" }),
        type === "target"
          ? `${model.filter((n) => isActive(model, n.id)).length} 个有效节点`
          : `${actual.filter((e) => e.kind === "directory").length} 个目录 · 文件显示在节点内`,
        type === "target" &&
          model.some((n) => !isActive(model, n.id)) &&
          h(
            "span",
            { className: "warning" },
            ` / ${model.filter((n) => !isActive(model, n.id)).length} 个未连接`,
          ),
      ),
      h(
        "div",
        { className: "toolbar-actions" },
        h(
          "select",
          {
            "aria-label": "定位目录",
            defaultValue: "",
            onChange: (e) => {
              const n = visual.find((n) => n.id === e.target.value);
              if (n)
                flow?.setCenter(n.position.x + (dimensions.get(n.id)?.width || 360) / 2, n.position.y + (dimensions.get(n.id)?.height || 138) / 2, {
                  zoom: 1,
                  duration: 250,
                });
            },
          },
          h("option", { value: "" }, "定位目录…"),
          ...visual.map((n) =>
            h("option", { key: n.id, value: n.id }, n.data.node.name),
          ),
        ),
        h(
          "button",
          { onClick: () => setFull(!full) },
          full ? "退出全屏" : "全屏画布",
        ),
        h(
          "button",
          {
            type: "button",
            title: "收起参数和子目录分支，保留当前节点的文件列表",
            onClick: () => {
              const ids = ["root", ...dirs.map((n) => n.id)];
              setExpandedParams(new Set());
              setBranchOverrides(new Map(ids.map((id) => [id, true])));
              setLayoutKey((n) => n + 1);
            },
          },
          "全部收起",
        ),
        h(
          "button",
          {
            type: "button",
            title: "仅展开当前可见节点的下一层，保留更深层的折叠状态和参数状态",
            onClick: () => {
              setBranchOverrides((previous) => {
                const next = new Map(previous);
                for (const n of visual) next.set(n.id, false);
                return next;
              });
              setLayoutKey((n) => n + 1);
            },
          },
          "展开一层",
        ),
        type !== "target" &&
          h(
            React.Fragment,
            null,
            h(
              "select",
              {
                "aria-label": "文件排序",
                value: sort,
                onChange: (e) => setSort(e.target.value),
              },
              h("option", { value: "name" }, "名称排序"),
              h("option", { value: "format" }, "格式排序"),
              h("option", { value: "time" }, "最近修改"),
              h("option", { value: "size" }, "大文件优先"),
            ),
            h(
              "label",
              { className: "check-label" },
              h("input", {
                type: "checkbox",
                checked: group,
                onChange: (e) => setGroup(e.target.checked),
              }),
              "按格式分组",
            ),
          ),
        editable &&
          h(
            "button",
            { onClick: (e) => setMenu({ x: e.clientX, y: e.clientY }) },
            "+ 新建节点",
          ),
        editable &&
          h(
            "button",
            {
              disabled: !selectedNodeIds.size,
              onClick: () => deleteSelection(),
              title: "删除分类节点，子节点保留为未连接节点；不删除磁盘文件",
            },
            "删除所选节点",
          ),
        editable &&
          selectedEdges.size > 0 &&
          h(
            "button",
            { onClick: () => deleteSelection(new Set()) },
            "断开所选连线",
          ),
        h(
          "button",
          {
            onClick: () => {
              setLayoutKey((n) => n + 1);
            },
          },
          "自动排列",
        ),
        h(
          "button",
          { onClick: fitOverview },
          "适应画布",
        ),
      ),
    ),
    h(
      "div",
      { className: "graph-canvas" },
      h(
        ReactFlow,
        {
          nodes: visual,
          edges,
          nodeTypes,
          onInit: setFlow,
          fitView: false,
          defaultViewport: { x: 28, y: 28, zoom: 1 },
          minZoom: 0.12,
          maxZoom: 2,
          fitViewOptions: { padding: 0.16, maxZoom: 1 },
          onlyRenderVisibleElements: true,
          deleteKeyCode: null,
          proOptions: { hideAttribution: true },
          onNodesChange: (changes) => {
            const safe = changes.filter((c) => c.type !== "position");
            setVisual((nodes) => applyNodeChanges(safe, editable ? dragDetached(nodes, model, changes) : nodes));
            if (safe.some((c) => c.type === "dimensions" && c.dimensions))
              setDimensions((previous) => {
                const next = new Map(previous);
                let changed = false;
                for (const c of safe) {
                  if (c.type !== "dimensions" || !c.dimensions) continue;
                  const old = previous.get(c.id);
                  if (old?.height !== c.dimensions.height || old?.width !== c.dimensions.width) {
                    next.set(c.id, c.dimensions);
                    changed = true;
                  }
                }
                return changed ? next : previous;
              });
          },
          onNodeDragStop: (_, node) => {
            const item = current.current.find((n) => n.id === node.id);
            if (editable && item && !item.parent && validPosition([node.position.x, node.position.y])) {
              update(node.id, {position: [node.position.x, node.position.y]});
            }
          },
          onNodeClick: (e, n) => {
            if (!e.target.closest("input,textarea,select,button,a"))
              selectNode(n.id);
          },
          onEdgeClick: (_, edge) => {
            setVisual((nodes) => nodes.map((n) => ({ ...n, selected: false })));
            setSelectedEdges(new Set([edge.id]));
            container.current?.focus({ preventScroll: true });
          },
          onEdgesChange: (changes) =>
            setSelectedEdges((previous) => {
              const next = new Set(previous);
              for (const c of changes)
                if (c.type === "select")
                  c.selected ? next.add(c.id) : next.delete(c.id);
              return next;
            }),
          onConnect: editable ? reconnect : undefined,
          isValidConnection: (c) =>
            editable && validConnection(model, c.source, c.target),
          onNodeContextMenu: editable
            ? (e, n) => {
                e.preventDefault();
                selectNode(n.id);
                setMenu({ x: e.clientX, y: e.clientY, nodeId: n.id });
              }
            : undefined,
          onPaneContextMenu: editable
            ? (e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY });
              }
            : undefined,
          onPaneClick: () => {
            setMenu(null);
            setSelectedEdges(new Set());
            setVisual((nodes) => nodes.map((n) => ({ ...n, selected: false })));
          },
          elementsSelectable: true,
          nodesDraggable: false,
          nodesConnectable: editable,
          edgesUpdatable: false,
        },
        h(Background, { color: "#c3d0cb", gap: 22, size: 1 }),
        h(Controls, { showInteractive: false, onFitView: fitOverview }),
        !compact &&
          h(MiniMap, {
            nodeColor: (n) => (n.data.orphan ? "#d4b478" : n.data.color),
            maskColor: "rgba(244,247,245,.8)",
            pannable: true,
            zoomable: true,
          }),
      ),
    ),
    h(
      "div",
      { className: "graph-hint" },
      editable
        ? "父节点在上、子节点横向成行 · 拖动孤立节点标题移动整棵子树 · 默认显示至三级 · 空白处平移 / 右键新建 · Delete 删除所选，不删除文件"
        : canClass
          ? "从上向下分行布局 · 默认显示至三级，大型和整体保护分支收起 · 参数中调整目录类型 · 拖动空白平移"
          : "从上向下分行布局 · 默认显示至三级，深层按需展开 · 拖动空白平移 · 文件列表保留在节点内",
    ),
    menu &&
      h(
        "div",
        {
          className: "context-menu",
          style: {
            left: Math.min(menu.x, innerWidth - 200),
            top: Math.min(menu.y, innerHeight - 90),
          },
        },
        menu.nodeId
          ? h(
              "button",
              {
                disabled: menu.nodeId === "root",
                onClick: () => deleteSelection(new Set([menu.nodeId])),
              },
              "删除这个节点",
            )
          : h("button", { onClick: () => add() }, "+ 创建未连接节点"),
        h(
          "small",
          null,
          menu.nodeId
            ? "仅删除分类节点，不删除磁盘文件"
            : "连接到根目录后参与整理",
        ),
      ),
  );
}
