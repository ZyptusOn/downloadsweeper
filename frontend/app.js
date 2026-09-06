import { Graph, sizeLabel, classLabel, projectedEntries } from "./graph.js";
import { Permissions } from "./permissions.js";
import { proposalDependencies, validProposalSelection } from "./proposal_selection.js";
import { parallelSummary } from "./parallel_progress.js";
const { createElement: h, useState, useEffect, useRef, useCallback, useId } = React;
function costTotals(calls = []) {
  const total = { currencies: {}, unknown: 0, estimated: 0, legacy: 0 };
  for (const call of calls) {
    const bill = call.billing;
    if (bill && Number.isFinite(bill.amount) && bill.currency) {
      total.currencies[bill.currency] = (total.currencies[bill.currency] || 0) + bill.amount;
      if (bill.status === "estimated") total.estimated++;
    } else if (!bill && Number.isFinite(call.cost_usd) && call.cost_usd > 0) {
      total.currencies.USD = (total.currencies.USD || 0) + call.cost_usd;
      total.legacy++;
    } else total.unknown++;
  }
  return total;
}
function costText(total) {
  if (!total) return "价格未知";
  const parts = Object.entries(total.currencies || {}).sort().map(([c, v]) => `${c === "CNY" ? "¥" : "$"}${v.toFixed(5)}`);
  if (total.unknown) parts.push(`${total.unknown} 次价格未知`);
  if (total.estimated) parts.push(`${total.estimated} 次缺缓存明细`);
  if (total.legacy) parts.push(`${total.legacy} 次历史单价`);
  return parts.join(" · ") || "尚无调用";
}
function callCost(call) {
  const b = call.billing;
  const label = b ? ({official:"官方价估算", manual:"手动价", estimated:"估算", unknown:"价格未知"}[b.status] || b.status) : "历史记录";
  return h("div", null, costText(costTotals([call])), h("small", null, label), b?.rates && h("small",null,`每百万 token：输入 ${b.rates.input} · 缓存读 ${b.rates.cached} · 输出 ${b.rates.output} ${b.currency}`), b?.source?.startsWith("https://") && h("a", {href:b.source,target:"_blank",rel:"noreferrer"}, "价格依据"), b && h("small", {title:b.note}, `${b.verified_at ? "核对 " + b.verified_at + " · " : ""}${b.note}`));
}
const STEPS = [
  ["扫描目录", "建立文件快照"],
  ["读取权限", "决定 AI 能看什么"],
  ["目标结构", "搭建你的分类"],
  ["生成计划", "规则与 AI 协作"],
  ["审查结果", "对比每一项改动"],
  ["执行整理", "记录与恢复"],
];
const STATUS = {
  draft: "规则草稿",
  planned: "计划已就绪",
  executing: "正在执行",
  completed: "整理完成",
  partial: "部分完成",
  rolled_back: "已恢复",
  recovery_required: "需检查恢复记录",
};
const TIERS = {
  none: "仅扩展名 · 不发给 AI",
  filename_only: "文件名",
  metadata: "文件名与元数据",
  image: "元数据与图片／视频／PDF 预览",
  content_slice: "文本／Office 切片与视觉预览",
};
const displayPath = (p) => String(p || "").replace(/^\\\\\?\\/, "");
const hasSampledFingerprints = (task) =>
  task.operations.some((o) => /^blake3-adaptive-v[12]:/.test(o.fingerprint || ""));
function fingerprintLabel(value) {
  if (!value) return "";
  if (value.startsWith("blake3-adaptive-v2:")) return "BLAKE3 分段 · > 1 MiB 抽样";
  if (value.startsWith("blake3-adaptive-v1:")) return "BLAKE3 分段 · > 16 MiB 抽样（旧规则）";
  if (value.startsWith("blake3:")) return "BLAKE3 全量";
  if (value.startsWith("sha256:") || /^[a-fA-F0-9]{64}$/.test(value)) return "SHA-256 全量 · 旧记录";
  return "未知指纹版本";
}
function Icon({ name, size = 18 }) {
  const paths = {
    folder: "M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z",
    arrow: "M4 12h15m-6-6 6 6-6 6",
    check: "m5 12 4 4L19 6",
    scan: "M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5M7 12h10",
    shield: "m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6Zm-4 9 3 3 5-6",
    tree: "M5 3v18m0-15h9m-9 12h9m0-14h6v4h-6Zm0 12h6v4h-6Z",
    history: "M3 12a9 9 0 1 0 3-7L3 8m0-5v5h5m4-1v6l4 2",
    settings:
      "m9 3-.5 2.4-1.4.8-2.3-.7-3 5.2L3.6 12l-1.8 1.3 3 5.2 2.3-.7 1.4.8L9 21h6l.5-2.4 1.4-.8 2.3.7 3-5.2L20.4 12l1.8-1.3-3-5.2-2.3.7-1.4-.8L15 3Zm3 5.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z",
    close: "m6 6 12 12M6 18 18 6",
    download: "M12 3v12m-5-5 5 5 5-5M4 17v4h16v-4",
    refresh: "M20 7a8 8 0 1 0 1 8M20 3v5h-5",
    spark: "m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5Z",
    play: "m8 4 12 8-12 8Z",
    back: "m14 5-7 7 7 7",
    file: "M6 3h8l4 4v14H6Zm8 0v5h5M9 12h6m-6 4h6",
    max: "M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5",
    minus: "M5 12h14",
  };
  return h(
    "svg",
    {
      width: size,
      height: size,
      viewBox: "0 0 24 24",
      fill: "none",
      stroke: "currentColor",
      strokeWidth: 1.6,
      strokeLinecap: "round",
      strokeLinejoin: "round",
      "aria-hidden": true,
    },
    h("path", { d: paths[name] || paths.folder }),
  );
}
const Button = ({ children, icon, kind = "", ...props }) =>
  h(
    "button",
    { ...props, className: "btn " + kind + " " + (props.className || "") },
    icon && h(Icon, { name: icon }),
    children,
  );
const Badge = ({ children, kind = "" }) =>
  h("span", { className: "badge " + kind }, children);
const Field = ({ label, hint, children }) => {
  const id=useId(); let bound=false;
  const controls=React.Children.map(children,function bind(child) {
    if (!React.isValidElement(child)) return child;
    if (!bound && ["input","select","textarea"].includes(child.type)) {
      bound=true;return React.cloneElement(child,{id,"aria-describedby":hint?id+"-hint":undefined});
    }
    return child.props.children ? React.cloneElement(child,{},React.Children.map(child.props.children,bind)) : child;
  });
  return h("div",{className:"field"},h("label",{htmlFor:id},label),controls,hint && h("small",{id:id+"-hint"},hint));
};

function Modal({ title, onClose, children, wide = false }) {
  const ref = useRef(null);
  useEffect(() => {
    const old = document.activeElement;
    ref.current?.focus();
    const key = (e) => {
      if (e.key === "Escape") onClose();
      if (e.key === "Tab") {
        const els = [
          ...ref.current.querySelectorAll(
            'button,input,select,textarea,[tabindex="0"]',
          ),
        ].filter((e) => !e.disabled);
        if (e.shiftKey && document.activeElement === els[0]) {
          e.preventDefault();
          els.at(-1)?.focus();
        } else if (!e.shiftKey && document.activeElement === els.at(-1)) {
          e.preventDefault();
          els[0]?.focus();
        }
      }
    };
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("keydown", key);
      old?.focus();
    };
  }, []);
  return h(
    "div",
    {
      className: "modal-backdrop",
      onMouseDown: (e) => {
        if (e.target === e.currentTarget) onClose();
      },
    },
    h(
      "div",
      {
        className: "modal " + (wide ? "wide" : ""),
        role: "dialog",
        "aria-modal": true,
        "aria-label": title,
        tabIndex: -1,
        ref,
      },
      h(
        "header",
        null,
        h("h2", null, title),
        h(Button, {
          kind: "icon-only",
          icon: "close",
          onClick: onClose,
          "aria-label": "关闭",
        }),
      ),
      children,
    ),
  );
}
function download(name, data) {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }),
  );
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
function useFormats(task) {
  const map = new Map();
  for (const f of task.entries.filter((e) => e.kind === "file")) {
    if (!map.has(f.extension))
      map.set(f.extension, { ext: f.extension, count: 0, size: 0 });
    const v = map.get(f.extension);
    v.count++;
    v.size += f.size;
  }
  return [...map.values()].sort((a, b) => b.count - a.count);
}

function App() {
  const [boot, setBoot] = useState(null),
    [task, setTask] = useState(null),
    [page, setPage] = useState("home"),
    [job, setJob] = useState(null),
    [lastJob, setLastJob] = useState(null),
    [savedJobs, setSavedJobs] = useState([]),
    [history, setHistory] = useState([]),
    [error, setError] = useState(""),
    [toast, setToast] = useState(""),
    [working, setWorking] = useState(false),
    [assistantOpen, setAssistantOpen] = useState(false),
    [contextPrompt, setContextPrompt] = useState(""),
    [sceneOverride, setSceneOverride] = useState(null);
  const token = useRef(""),
    taskRef = useRef(task),
    queue = useRef(Promise.resolve()),
    lastSeen = useRef("");
  taskRef.current = task;
  const read = async (path) => {
    const r = await fetch(path);
    const body = await r.json();
    if (!r.ok) throw Error(body.error || "本地服务返回错误");
    return body;
  };
  const refreshHistory = () => read("/api/tasks").then(setHistory);
  const refreshJobs = () => read("/api/jobs").then(setSavedJobs);
  const receiveTask = (t) => {
    taskRef.current = t;
    setTask(t);
  };
  useEffect(() => {
    (async () => {
      try {
        const b = await read("/api/bootstrap");
        token.current = b.token;
        setBoot(b);
        setJob(b.job);
        setLastJob(b.last_job);
        await refreshJobs();
        await refreshHistory();
        const id = location.hash.match(/task=([a-f0-9-]+)/)?.[1];
        if (id) {
          receiveTask(await read("/api/tasks/" + id));
          setPage("workspace");
        }
      } catch (e) {
        setError(e.message);
      }
    })();
    const events = new EventSource("/api/events");
    events.onmessage = async (e) => {
      const v = JSON.parse(e.data);
      if (v.type === "progress") setJob(v.job);
      if (v.type === "finished") {
        setJob(null);
        setLastJob(v.job);
        if (v.job.error && v.job.status !== "paused") setError(v.job.error);
        else
          setToast(
            v.job.kind === "test_connection"
              ? "模型连接成功，实际用量已记录"
              : v.job.message || "任务已完成",
          );
        if (taskRef.current?.id === v.job.task_id)
          receiveTask(await read("/api/tasks/" + v.job.task_id));
        refreshHistory();
        refreshJobs();
      }
    };
    events.onerror = () => {};
    return () => events.close();
  }, []);
  useEffect(() => {
    if (!job) return;
    const timer = setInterval(async () => {
      try {
        const b = await read("/api/bootstrap");
        setJob(b.job);
        if (!b.job && b.last_job && lastSeen.current !== b.last_job.id) {
          lastSeen.current = b.last_job.id;
          setLastJob(b.last_job);
          refreshJobs();
          if (b.last_job.error && b.last_job.status !== "paused") setError(b.last_job.error);
        }
        if (taskRef.current?.id === job.task_id)
          receiveTask(await read("/api/tasks/" + job.task_id));
      } catch (e) {
        setError("本地服务连接中断，请检查 Rust 服务是否运行。");
      }
    }, 1800);
    return () => clearInterval(timer);
  }, [job?.id]);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(""), 4500);
    return () => clearTimeout(t);
  }, [toast]);
  function act(action, args = {}, id) {
    const run = async () => {
      setWorking(true);
      setError("");
      try {
        const current = taskRef.current;
        const payload = JSON.stringify({
          action,
          task_id: id || current?.id,
          revision: current?.revision,
          ...args,
        });
        const send = () =>
          fetch("/api/action", {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              "x-ds-token": token.current,
            },
            body: payload,
          });
        let response = await send();
        let value = await response.json();
        // This specific rejection happens in middleware before dispatch. Renew
        // the same-origin session and resend once; never retry an uncertain write.
        if (
          response.status === 403 &&
          (value.code === "session_expired" ||
            value.error === "页面会话已失效，请刷新后重试")
        ) {
          const fresh = await read("/api/bootstrap");
          token.current = fresh.token;
          setBoot(fresh);
          setJob(fresh.job);
          response = await send();
          value = await response.json();
        }
        if (!response.ok) throw Error(value.error || "操作失败");
        if (value.job) {
          setJob(value.job);
          setLastJob(null);
        } else if (value.schema_version === 2) { receiveTask(value); refreshJobs(); }
        return value;
      } catch (e) {
        setError(e.message);
        throw e;
      } finally {
        setWorking(false);
      }
    };
    const promise = queue.current.then(run);
    queue.current = promise.catch(() => {});
    return promise;
  }
  const doAct = (...args) => act(...args).catch(() => {});
  async function mergeProposal(proposal, ids, selected) {
    const updated = await act("proposal", {
      proposal_id: proposal.id,
      scene: proposal.scene,
      ids, selected,
    });
    setToast(
      proposal.scene === "review" ? "正在应用建议并规划文件位置，完成后更新整理后预览" : `已合并 ${ids.length} 项建议，${proposal.scene === "tree" ? "目录树" : "当前规则"}已更新`,
    );
    return updated;
  }
  const busy = !!job || working;
  async function create(root, mode = "organize") {
    const t = await act("create", { root, mode });
    location.hash = "task=" + t.id;
    setPage("workspace");
    return t;
  }
  async function open(id) {
    try {
      receiveTask(await read("/api/tasks/" + id));
      location.hash = "task=" + id;
      setPage("workspace");
      setLastJob(null);
    } catch (e) {
      setError(e.message);
    }
  }
  const pageTo = (p) => {
    setPage(p);
    if (p === "history") refreshHistory();
  };
  const scene =
    page === "workspace"
      ? sceneOverride ||
        ["scan", "permissions", "tree", "planning", "review", "execution"][
          task?.phase || 0
        ]
      : page;
  const ask = (message = "") => {
    setContextPrompt(message);
    setAssistantOpen(true);
  };
  async function sendChat(message, inspect = false) {
    if (!taskRef.current) {
      const root = boot.config.default_scan_root;
      await act("create", { root, mode: "organize" });
    }
    await act(inspect ? "suggest_tree" : "chat", { message, scene });
  }
  const total = task?.calls.reduce(
    (s, c) => ({
      input: s.input + c.usage.prompt_tokens,
      output: s.output + c.usage.completion_tokens,
      cost: s.cost + (c.cost_usd || 0),
    }),
    { input: 0, output: 0, cost: 0 },
  ) || { input: 0, output: 0, cost: 0 };
  total.costs = costTotals(task?.calls || []);
  return h(
    React.Fragment,
    null,
    h(
      "header",
      { className: "app-header" },
      h(
        "button",
        { className: "brand", onClick: () => pageTo("home") },
        h(
          "span",
          { className: "brand-mark" },
          h(Icon, { name: "folder", size: 23 }),
        ),
        h("span", null, "Download", h("b", null, "Sweeper")),
      ),
      h(
        "nav",
        null,
        h(
          "button",
          {
            className: ["home", "workspace"].includes(page) ? "active" : "",
            onClick: () => pageTo(task ? "workspace" : "home"),
          },
          h(Icon, { name: "tree" }),
          "整理工作台",
        ),
        h(
          "button",
          {
            className: page === "history" ? "active" : "",
            onClick: () => pageTo("history"),
          },
          h(Icon, { name: "history" }),
          "任务历史",
        ),
      ),
      h(
        "div",
        { className: "header-right" },
        !job && savedJobs.some(j => ["paused","interrupted","failed"].includes(j.status) || j.kind === "archive_export" && j.status === "completed") &&
          h(Button,{kind:"text",onClick:()=>{
            const panel=document.getElementById("run-checkpoints");
            if(panel){panel.open=true;panel.scrollIntoView({behavior:"smooth",block:"start"});}
          }},"运行检查点"),
        h(
          "span",
          { className: "local-status" },
          h("i"),
          boot ? "本地 Rust 服务" : "连接中",
        ),
        h(Button, {
          kind: "icon-only",
          icon: "settings",
          onClick: () => pageTo("settings"),
          "aria-label": "模型与设置",
        }),
      ),
    ),
    error &&
      h(
        "div",
        { className: "error-banner", role: "alert" },
        h("span", null, error),
        h(
          "button",
          { onClick: () => setError(""), "aria-label": "关闭错误提示" },
          "×",
        ),
      ),
    !boot
      ? h("main", { className: "boot" }, "正在连接本地工作台…")
      : h(
          "main",
          { className: "app-main " + page },
          page === "home" &&
            h(Home, {
              config: boot.config,
              history,
              busy,
              onCreate: (...a) => create(...a).catch(() => {}),
              onOpen: open,
              onSettings: () => pageTo("settings"),
            }),
          page === "workspace" &&
            task &&
            h(Workspace, {
              task,
              fingerprintPolicy: boot.fingerprint_policy,
              recycleSupported: boot.config.recycle_supported,
              permissionPresets: boot.config.permission_presets,
              act: doAct,
              onMerge: mergeProposal,
              busy,
              job,
              lastJob,
              onNodes: (nodes) => doAct("tree", { nodes }),
              onSceneChange: setSceneOverride,
              usage: total,
              budget: boot.config.token_budget,
              autoTemplate:
                boot.config.has_api_key ||
                /^http:\/\/(127\.0\.0\.1|localhost)(:|\/)/.test(
                  boot.config.llm.endpoint,
                ),
              ask,
              onNew: () => pageTo("home"),
              read,
            }),
          page === "settings" &&
            h(Settings, {
              config: boot.config,
              busy,
              onSave: async (config, clear_key, clear_search_key) => {
                const c = await act("config", {
                  config,
                  clear_key,
                  clear_search_key,
                });
                setBoot((b) => ({ ...b, config: c }));
                setToast("配置已保存");
                return c;
              },
              onPricing: async (config, signal) => {
                const response = await fetch("/api/pricing-preview", {method:"POST",headers:{"Content-Type":"application/json","x-ds-token":token.current},body:JSON.stringify(config),signal});
                const value = await response.json();
                if (!response.ok) throw new Error(value.error || "价格查询失败");
                return value;
              },
              onDiscover: async (connection, signal) => {
                const response = await fetch("/api/models", {
                  method: "POST", signal,
                  headers: { "Content-Type": "application/json", "x-ds-token": token.current },
                  body: JSON.stringify(connection),
                });
                const result = await response.json();
                if (!response.ok) throw Error(result.error || "查询模型失败，请刷新页面后重试");
                return result;
              },
              onTest: async () => {
                if (!taskRef.current)
                  await act("create", {
                    root: boot.config.default_scan_root,
                    mode: "organize",
                  });
                await act("test_connection");
              },
              onBack: () => pageTo(task ? "workspace" : "home"),
            }),
          page === "history" &&
            h(History, {
              tasks: history,
              busy,
              onOpen: open,
              onImport: async (value) => {
                try {
                  const t = await act(value.format === "downloadsweeper-archive" ? "archive_import" : "import", { task: value });
                  if (t.job) return t;
                  if (t.archive_id) { setToast("完整归档已只读保存，可在历史页查看"); return t; }
                  location.hash = "task=" + t.id;
                  setPage("workspace");
                  setToast("旧会话已导入，请重新扫描以校验本机文件");
                  return t;
                } catch {}
              },
              onExport: async (id) => {
                const snapshot = await read("/api/tasks/" + id);
                return act("archive_export", {revision:snapshot.revision}, id);
              },
              onResume: async (archiveId, root) => {
                const t = await act("resume_archive", {archive_id: archiveId, root});
                location.hash = "task=" + t.id; setPage("workspace");
                setToast("已创建新任务，请重新扫描；原归档保持不变");
              },
              read,
            }),
        ),
    !job && savedJobs.some(j => ["paused","interrupted","failed"].includes(j.status) || j.kind === "archive_export" && j.status === "completed") &&
      h("details", {id:"run-checkpoints",className:"recovery-panel",open:true},
        h("summary",null,"运行记录与检查点"),
        ...savedJobs.filter(j => ["paused","interrupted","failed"].includes(j.status) || j.kind === "archive_export" && j.status === "completed").slice(0,20).map(j=>
          h("article",{key:j.id},
            h("div",null,h("strong",null,j.message),
              h("small",null,`${new Date(j.saved_at).toLocaleString()} · ${j.kind.startsWith("archive_") && j.total ? `${sizeLabel(j.current)} / ${sizeLabel(j.total)}` : j.total ? `${j.current} / ${j.total}` : `已处理 ${j.current} 项`}`),
              h("p",null,j.recovery_note),j.error && h("small",{className:"muted"},j.error)),
            j.task_id !== "00000000-0000-0000-0000-000000000000" && h(Button,{disabled:busy,onClick:()=>open(j.task_id)},"查看任务"),
            j.resumable && h(Button,{disabled:busy,onClick:async()=>{
              const result = await doAct("resume_job",{job_id:j.id});
              if(result?.job && j.task_id !== "00000000-0000-0000-0000-000000000000") await open(j.task_id);
              refreshJobs();
            }},"从检查点继续"),
            j.kind === "archive_export" && j.status === "completed" && h(Button,{disabled:busy,onClick:async()=>{
              try { download("downloadsweeper-"+j.task_id+".json", await read("/api/jobs/"+j.id+"/result")); }
              catch(e){setError(e.message);}
            }},"下载完整归档")))),
    job &&
      h(
        "div",
        { className: "job-bar", role: "status", "aria-live": "polite" },
        h("span", { className: "spinner" }),
        h(
          "div",
          null,
          h("strong", null, ["plan_ai", "review_proposal"].includes(job.kind) && job.parallel && job.status !== "pausing" && parallelSummary(job.parallel).percent < 100 ? "并行分类 · 完成的批次持续保存" : job.message),
          job.parallel && h(ParallelProgress, {run:job.parallel, compact:true}),
          h(
            "small",
            null,
            job.parallel ? `${parallelSummary(job.parallel).completed} / ${parallelSummary(job.parallel).total} 个文件已完成 · ${Math.floor(parallelSummary(job.parallel).percent)}%` : job.total
              ? job.kind.startsWith("archive_") ? `${sizeLabel(job.current)} / ${sizeLabel(job.total)}` : `${job.current} / ${job.total}`
              : `正在处理中 · 已处理 ${job.current} 项`,
          ),
          h("small",null,job.status === "pausing" ? "正在到达安全保存点，请等待暂停完成" : `检查点保存于 ${new Date(job.saved_at).toLocaleTimeString()}`),
        ),
        h(
          "div",
          { className: "job-progress " + (!job.total ? "indeterminate" : "") },
          h("i", {
            style: {
              width: job.parallel ? `${parallelSummary(job.parallel).percent}%` : job.total
                ? `${Math.min(100, (100 * job.current) / job.total)}%`
                : undefined,
            },
          }),
        ),
        h(
          Button,
          {
            kind: "danger-soft",
            disabled: working || job.status === "pausing",
            onClick: () => doAct("cancel", { job_id: job.id }),
          },
          job.status === "pausing" ? "正在保存…" : "暂停并保存",
        ),
      ),
    boot &&
      h(Assistant, {
        open: assistantOpen,
        setOpen: setAssistantOpen,
        task,
        scene,
        prompt: contextPrompt,
        onPromptUsed: () => setContextPrompt(""),
        onSend: (msg, inspect) => sendChat(msg, inspect).catch(() => {}),
        busy,
        usage: total,
        budget: boot.config.token_budget,
        act: doAct,
        onMerge: mergeProposal,
      }),
    toast &&
      h(
        "div",
        { className: "toast", role: "status" },
        h(Icon, { name: "check" }),
        toast,
      ),
  );
}

function Home({ config, history, busy, onCreate, onOpen, onSettings }) {
  const [root, setRoot] = useState(displayPath(config.default_scan_root)),
    [mode, setMode] = useState("organize");
  return h(
    "div",
    { className: "home-wrap" },
    h(
      "div",
      { className: "home-kicker" },
      h("span", { className: "pill-dot" }),
      "为下载目录与桌面，建立清晰的秩序",
    ),
    h(
      "section",
      { className: "hero" },
      h(
        "div",
        { className: "hero-copy" },
        h(
          "h1",
          null,
          mode === "rename" ? "让难懂的文件名，" : "每一个文件，",
          h("br"),
          h("em", null, mode === "rename" ? "重新变得清楚。" : "都有自己的位置。"),
        ),
        h(
          "p",
          null,
          mode === "rename" ? "选择文件类别，让 AI 提议可读名称。" : "由你定义规则，让 AI 理解内容。",
          h("br"),
          mode === "rename" ? "逐项对比原名与新名，确认后原位重命名，保留目录结构。" : "从第一次扫描到最后一次移动，每一步都看得见、可掌控。",
        ),
        h(
          "div",
          { className: "hero-features" },
          h("span", null, h(Icon, { name: "shield" }), "隐私由你决定"),
          h("span", null, h(Icon, { name: mode === "rename" ? "spark" : "tree" }), mode === "rename" ? "原名与新名对比" : "可视化目录编排"),
          h("span", null, h(Icon, { name: "history" }), "每次操作可追溯"),
        ),
      ),
      h(
        "div",
        { className: "hero-art", "aria-hidden": true },
        h("div", { className: "art-caption" }, "FROM CLUTTER TO CLARITY"),
        h(
          "div",
          { className: "art-source" },
          h(Icon, { name: "folder", size: 25 }),
          h("b", null, mode === "rename" ? "IMG_0017.png" : "Downloads"),
          h("small", null, "所有灵感的起点"),
        ),
        h("div", { className: "art-lines" }),
        h(
          "div",
          { className: "art-targets" },
          ...(mode === "rename" ? ["合同扫描件.pdf", "课程笔记.txt", "产品设计草图.png"] : ["文档 / 工作资料", "视频 / 电影", "图片 / 设计素材"]).map(
            (label, i) =>
              h(
                "div",
                { key: label, className: "art-folder art-" + i },
                h(Icon, { name: "folder" }),
                label,
                h("span", null, "↗"),
              ),
          ),
        ),
        h("span", { className: "art-note" }, mode === "rename" ? "名称示意 · 实际建议由你审查" : "规则负责秩序，AI 负责理解"),
      ),
    ),
    h(
      "section",
      { className: "start-card" },
      h("div", {className: "tabs mode-tabs", "aria-label": "整理模式"},
        ...[["organize", "下载目录整理"], ["desktop", "桌面整理"], ["rename", "文件名重生"]].map(([value, label]) =>
          h("button", {key: value, className: mode === value ? "active" : "", disabled: busy,
            "aria-pressed": mode === value,
            onClick: () => { setMode(value); setRoot(displayPath(value === "desktop" ? (config.default_desktop_root || "") : config.default_scan_root)); }
          }, label))),
      h(
        "div",
        { className: "start-title" },
        h(
          "div",
          null,
          h("h2", null, mode === "rename" ? "选择需要修复文件名的目录" : "从一个目录开始"),
          h("p", null, mode === "desktop" ? "轻量扫描桌面，保护已有文件夹。可编辑目标树，由 Agent 按权限理解文件并制定整理计划。" : mode === "rename" ? "扫描后按格式选择范围，设置读取权限，再生成新名称。需配置模型；联网检索可选。" : "先扫描，再决定如何整理。文件会在最后确认后才移动。"),
        ),
        h(Badge, null, "本地优先"),
      ),
      h(
        "div",
        { className: "start-form" },
        h(
          "div",
          { className: "path-input" },
          h(Icon, { name: "folder" }),
          h("input", {
            "aria-label": "要整理的目录",
            value: root,
            onChange: (e) => setRoot(e.target.value),
            placeholder: mode === "desktop" ? "输入桌面或演示目录的完整路径" : "输入下载目录的完整路径",
          }),
        ),
        h(
          Button,
          {
            kind: "primary",
            icon: "arrow",
            disabled: busy || !root.trim(),
            onClick: () => onCreate(root, mode),
          },
          mode === "rename" ? "进入文件名重生" : mode === "desktop" ? "开始桌面整理" : "开始整理",
        ),
      ),
      h(
        "div",
        { className: "start-options" },
        h("span", null, "支持 Windows 与 macOS · 路径可直接粘贴"),
        h("span", null, mode === "desktop" ? "轻量扫描 · 自定义目录树 · 完整 Agent 协作" : mode === "rename" ? "按格式选范围 · 原位重命名 · 可恢复原名" : "文件只在最后确认后移动"),
      ),
    ),
    h(
      "section",
      { className: "journey" },
      h(
        "div",
        { className: "section-heading" },
        h("h2", null, mode === "rename" ? "文件名重生，从选择范围到恢复原名" : "清晰的六步，始终由你掌控"),
        h("span", null, "一次任务，完整记录"),
      ),
      h(
        "div",
        { className: "journey-grid" },
        ...(mode === "rename" ? [["扫描文件", "建立待命名文件快照"], ["读取权限", "决定 AI 可读的名称与内容"], ["命名范围", "选择格式与可选联网检索"], ["生成新名", "AI 提议易读名称"], ["对比审查", "逐项选择原名 → 新名"], ["执行重命名", "原位改名，可恢复原名"]] : STEPS).map(([name, desc], i) =>
          h(
            "div",
            { key: name, className: "journey-item" },
            h("span", null, String(i).padStart(2, "0")),
            h("h3", null, name),
            h("p", null, desc),
          ),
        ),
      ),
    ),
    h(
      "div",
      { className: "home-bottom" },
      history.length
        ? h(
            "div",
            null,
            h("h3", null, "继续上次整理"),
            h(
              "button",
              {
                className: "recent-task",
                onClick: () => onOpen(history[0].id),
              },
              h(Icon, { name: "history" }),
              h("span", null, displayPath(history[0].root)),
              h(Badge, null, STATUS[history[0].status]),
              h(Icon, { name: "arrow" }),
            ),
          )
        : h(
            "div",
            null,
            h("h3", null, "准备好你的 AI 助手"),
            h("p", null, mode === "rename" ? "配置 API Endpoint、Key 与模型，生成新名称前会按你的读取权限提供信息。" : "配置自己的兼容 API，或先使用本地规则体验完整流程。"),
          ),
      h(Button, { icon: "settings", onClick: onSettings }, "模型与连接设置"),
    ),
  );
}

function Workspace({
  task,
  fingerprintPolicy,
  recycleSupported,
  permissionPresets,
  act,
  onMerge,
  busy,
  job,
  lastJob,
  onNodes,
  onSceneChange,
  usage,
  budget,
  autoTemplate,
  ask,
  onNew,
  read,
}) {
  const [permissionDirty, setPermissionDirty] = useState(false),
    [tab, setTab] = useState("target"),
    [examples, setExamples] = useState(null),
    [logs, setLogs] = useState(null),
    [confirm, setConfirm] = useState(null),
    [selected, setSelected] = useState(
      new Set(task.operations.filter((o) => o.selected).map((o) => o.id)),
    ),
    [reviewed, setReviewed] = useState(task.reviewed);
  const [classificationBatchSize, setClassificationBatchSize] = useState(
    task.classification?.batch_size ?? 256,
  );
  const [classificationThinking, setClassificationThinking] = useState(
    task.classification?.thinking ?? false,
  );
  useEffect(() => {
    setClassificationBatchSize(task.classification?.batch_size ?? 256);
    setClassificationThinking(task.classification?.thinking ?? false);
  }, [task.id]);
  const invalidBatchSize =
    !Number.isInteger(Number(classificationBatchSize)) ||
    Number(classificationBatchSize) < 1 ||
    Number(classificationBatchSize) > 1024;
  const readiness = task.classification_readiness;
  const noAiFiles = readiness?.eligible_files === 0;
  const readinessView = readiness && h(
    "div", {className:"classification-readiness", role:"status"},
    h("strong", null, noAiFiles ? "当前没有可交给 AI 分类的条目" : `${readiness.eligible_files} 个条目可交给 AI 分类（文件或完整文件夹）`),
    h("p", null, [
      readiness.no_semantic_rule && `${readiness.no_semantic_rule} 个文件的目标分支没有复杂规则子目录`,
      readiness.no_matching_rule && `${readiness.no_matching_rule} 个文件未匹配一级规则`,
      readiness.permission_denied && `${readiness.permission_denied} 个文件未获准读取文件名`,
      readiness.protected_entries && `${readiness.protected_entries} 个特殊文件保持原位`,
      readiness.already_mapped && `${readiness.already_mapped} 个文件已在复用目录中`,
    ].filter(Boolean).join("；") || "将按当前目标树和读取权限进行分类。"),
    (readiness.no_semantic_rule > 0 || readiness.no_matching_rule > 0) && h("p", null,
      "AI 批量分类需要已有的复杂规则子目录。可返回目标结构添加子目录，或生成并合并 AI 目录建议。"),
    noAiFiles && h("small", null, "尚未调用 AI，不产生 API 用量；基础规则计划仍可直接审查。"),
    (readiness.no_semantic_rule > 0 || readiness.no_matching_rule > 0) && h(Button,
      {disabled:busy, onClick:()=>back(2)}, "返回目标结构，补充分类"),
    readiness.permission_denied > 0 && h(Button,
      {disabled:busy, onClick:()=>back(1)}, "返回读取权限"),
  );
  const classificationControls = h(ClassificationControls, {
    batchSize: classificationBatchSize,
    setBatchSize: setClassificationBatchSize,
    thinking: classificationThinking,
    setThinking: setClassificationThinking,
    disabled: busy,
  });
  useEffect(() => {
    onSceneChange(task.phase === 2 && tab === "actual" ? "directories" : null);
    return () => onSceneChange(null);
  }, [task.phase, tab, task.mode]);
  const formats = useFormats(task),
    files = task.entries.filter((e) => e.kind === "file"),
    dirs = task.entries.filter((e) => e.kind === "directory"),
    locked = [
      "completed",
      "partial",
      "rolled_back",
      "recovery_required",
      "executing",
    ].includes(task.status);
  useEffect(() => {
    setSelected(
      new Set(task.operations.filter((o) => o.selected).map((o) => o.id)),
    );
    setReviewed(task.reviewed);
  }, [task.id, JSON.stringify(task.operations), task.reviewed]);
  useEffect(() => {
    window.scrollTo({ top: 0, behavior: "instant" });
  }, [task.phase, task.id]);
  const phase = task.phase;
  const desktop = task.mode === "desktop";
  const planReady = task.status === "planned";
  const autoPlanTask = useRef(null);
  useEffect(() => {
    if (phase !== 3) {
      autoPlanTask.current = null;
      return;
    }
    if (
      !["organize", "desktop"].includes(task.mode) ||
      planReady ||
      busy ||
      autoPlanTask.current === task.id
    )
      return;
    // Resume older drafts opened directly in this stage, without retrying cancelled/failed jobs.
    autoPlanTask.current = task.id;
    if (
      lastJob?.task_id === task.id &&
      lastJob.kind === "plan_rules" &&
      ["failed", "cancelled", "paused", "interrupted"].includes(lastJob.status)
    )
      return;
    act("plan_rules");
  }, [task.id, phase, planReady, busy, lastJob?.id]);
  const steps = STEPS.map((x) => [...x]);
  if (task.mode === "rename") steps[2] = ["命名范围", "按类别选择文件"];
  const setClass = (id, value) => act("directory", { id, class: value });
  const graphProps = {
    task,
    onClass: busy || permissionDirty ? undefined : setClass,
  };
  async function next() {
    if (phase === 4) {
      const saved = await act("review", { selected: [...selected], reviewed });
      if (saved) await act("advance");
    } else {
      if (phase === 1) setTab("target");
      const advanced = await act("advance");
      if (
        advanced &&
        phase === 1 &&
        ["organize", "desktop"].includes(task.mode) &&
        autoTemplate &&
        !task.messages.some((m) => m.scene === "tree")
      ) {
        await act("suggest_tree", {
          scene: "tree",
          message:
            "请在本地模板基础上，结合分类型检查摘要设计初始多层目录结构：复用合适的分类，为已识别用途补充语义子目录和分类描述。请将新增、修改或删除节点以可合并差异返回。",
        });
      }
    }
  }
  function back(to) {
    if (to <= 2 && planReady)
      setConfirm({
        title: "返回修改规则？",
        body: "当前具体计划将失效。已经扫描的文件和本次 AI 用量记录会保留，修改后需要重新生成计划。",
        run: () => act("back", { phase: to }),
      });
    else act("back", { phase: to });
  }
  async function refinePlan() {
    if (phase === 4) {
      const saved = await act("review", {
        selected: [...selected],
        reviewed: false,
      });
      if (!saved) return;
    }
    await act("plan_ai", {
      batch_size: Number(classificationBatchSize),
      thinking: classificationThinking,
    });
  }
  return h(
    "div",
    { className: "workspace-wrap" },
    h(
      "div",
      { className: "workspace-heading" },
      h(
        "div",
        null,
        h("div", { className: "eyebrow" }, "ORGANIZE WORKSPACE"),
        h(
          "h1",
          null,
          task.mode === "rename" ? "文件名重生" : desktop ? "桌面整理" : "整理工作台",
          h(Badge, { kind: locked ? "mint" : "" }, STATUS[task.status]),
        ),
      ),
      h(
        "div",
        { className: "workspace-tools" },
        h(
          Button,
          {
            icon: "history",
            onClick: async () =>
              setLogs(await read("/api/tasks/" + task.id + "/trajectory")),
          },
          "任务轨迹",
        ),
        h(
          Button,
          {
            icon: "download",
            disabled: busy,
            onClick: async () => {
              try { await act("archive_export"); }
              catch (e) { alert(e.message); }
            },
          },
          "完整归档",
        ),
        h(
          Button,
          { icon: "folder", disabled: busy, onClick: onNew },
          "新建任务",
        ),
      ),
    ),
    h(
      "div",
      { className: "workspace-path" },
      h(Icon, { name: "folder", size: 15 }),
      h("span", { title: displayPath(task.root) }, displayPath(task.root)),
      h("span", null, "任务 " + task.id.slice(0, 8)),
    ),
    h(
      "div",
      { className: "task-usage", "aria-live": "polite" },
      h(
        "span",
        null,
        "输入 ",
        h("b", null, usage.input.toLocaleString()),
        " token",
      ),
      h(
        "span",
        null,
        "输出 ",
        h("b", null, usage.output.toLocaleString()),
        " token",
      ),
      h("span", null, "模型费用估算 ", h("b", null, costText(usage.costs))),
      h(
        "span",
        null,
        "预算 ",
        h(
          "b",
          null,
          budget
            ? `${(usage.input + usage.output).toLocaleString()} / ${budget.toLocaleString()}`
            : "未设上限",
        ),
      ),
      budget &&
        h("meter", {
          min: 0,
          max: budget,
          value: Math.min(budget, usage.input + usage.output),
          "aria-label": "任务 token 预算用量",
        }),
      !!task.search_calls?.length &&
        h("span", null, `搜索 ${task.search_calls.length} 次 · 费用另计`),
    ),
    !busy && task.pending_calls?.length > 0 && h("div", {className:"notice"},
      `有 ${task.pending_calls.length} 次模型请求未确认完整用量，保留 ${task.pending_calls.reduce((n,c)=>n+(c.reserved_tokens||0),0).toLocaleString()} token 预留。这不是实测消耗；继续时仍计入预算，可在模型设置中增加任务预算。`),
    task.mode === "rename" && task.rename_checkpoint && h("div", {className:"notice"},
      `文件名建议已保存 ${Object.keys(task.rename_checkpoint.results || {}).length}/${task.rename_checkpoint.total || 0}。失败或取消后可继续，原计划保留至本轮完成。`),
    h(
      "ol",
      { className: "stepper" },
      ...[0, 1, 2, 3, 4, 5].map((i) => {
        const [name, desc] = steps[i];
        return (
        h(
          "li",
          {
            key: i,
            className: i === phase ? "current" : i < phase ? "done" : "pending",
            "aria-current": i === phase ? "step" : undefined,
          },
          h(
            "button",
            { disabled: busy || i >= phase || locked, onClick: () => back(i) },
            h(
              "span",
              { className: "step-number" },
              i < phase
                ? h(Icon, { name: "check", size: 15 })
                : String(i).padStart(2, "0"),
            ),
            h("span", null, h("b", null, name), h("small", null, desc)),
          ),
        )); },
      ),
    ),
    h(
      "div",
      { className: "stage-heading" },
      h(
        "div",
        null,
        h(
          "h2",
          null,
          phase === 0
            ? desktop ? "只整理桌面上散落的文件" : "先认识这个目录"
            : phase === 1
              ? "给 AI 恰到好处的权限"
              : phase === 2
                ? task.mode === "rename"
                  ? "选择要重生的文件类别"
                  : "把你的整理思路，连成一棵树"
                : phase === 3
                  ? "让规则和 AI 一起制定计划"
                  : phase === 4
                    ? "看看每个文件将去哪里"
                    : locked
                      ? "每一次改动，都有来路"
                      : "准备好，让目录焕然一新",
        ),
        h(
          "p",
          null,
          phase === 0
            ? desktop ? "扫描只枚举第一层；AI 可按权限识别已有文件夹并整体归类，不拆散内部文件。接下来设置读取权限并编辑目标树。" : "扫描只收集名称、大小和目录结构，不读取内容。"
            : phase === 1
              ? desktop ? "设置文件与文件夹的读取权限；普通文件夹可整体移动，复用容器保留原位接收新文件，均不拆散；快捷方式与系统文件保留。" : "权限由 Rust 在发送给模型前强制过滤。目录节点中可设置整体保护或复用。"
              : phase === 2
                ? "一级按扩展名分流，子级理解语义。参数、备注和文件示例都可以在节点内编辑。"
                : phase === 3
                  ? task.mode === "rename"
                    ? "生成可读文件名，再逐项审查重命名计划。"
                    : "基础规则计划自动生成；按需用 AI 细化到子目录，准备好后点击下一步审查。"
                  : phase === 4
                    ? "左侧保留原始快照，右侧展示所选操作完成后的完整结构。"
                    : locked
                      ? "查看操作记录，或按保存的校验规则把本次已移动的文件恢复到原位。"
                      : "将按已确认的目标位置逐项执行。每项移动前后都会写入本地记录。",
        ),
      ),
      phase === 2 &&
        ["organize", "desktop"].includes(task.mode) &&
        h(
          Button,
          {
            icon: "spark",
            disabled: busy,
            onClick: () => {
              if (tab === "actual")
                ask(
                  "请检查当前实际目录的整体保护与复用类型，以可合并改动给出建议。",
                );
              else {
                ask();
                act("suggest_tree", {
                  scene: "tree",
                  message:
                    "请根据已有检查摘要逐步设计多层目录结构：本轮优先补齐扫描中数量较多但模板缺失的一级格式类别（例如音频），再细化1至2个分支，最多改动12个节点；可同批新增一级目录及其语义子目录，完善分类描述和边界。请直接生成可合并改动，其他分支可以后续继续。",
                });
              }
            },
          },
          tab === "actual"
            ? "让 AI 检查目录类型"
            : task.inspection?.status === "complete"
              ? "根据分析建议结构"
              : ["paused", "failed"].includes(task.inspection?.status)
                ? "继续检查并建议结构"
                : "分类型检查并建议结构",
        ),
    ),
    phase === 0 &&
      h(
        React.Fragment,
        null,
        !task.scanned
          ? h(
              "section",
              { className: "scan-start" },
              h(
                "div",
                { className: "scan-symbol" },
                h(Icon, { name: "scan", size: 42 }),
              ),
              h("h3", null, "从一次只读扫描开始"),
              h(
                "p",
                null,
                desktop ? "初始扫描只枚举第一层；AI 可按权限抽样识别文件夹并整体归类，内部不会拆分。" : "识别目录、统计格式，并找出需要整体保护的软件与素材库。",
              ),
              h(
                Button,
                {
                  kind: "primary",
                  icon: "scan",
                  disabled: busy,
                  onClick: () => act("scan"),
                },
                "扫描这个目录",
              ),
            )
          : h(
              React.Fragment,
              null,
              h(Stats, {
                items: [
                  [files.length, "个文件"],
                  [dirs.length, "个目录"],
                  [sizeLabel(files.reduce((s, e) => s + e.size, 0)), "总大小"],
                  [
                    dirs.filter((e) => e.class === "atomic").length,
                    "个整体保护目录",
                  ],
                ],
              }),
              h(Graph, { ...graphProps }),
              task.warnings.length > 0 &&
                h(
                  "details",
                  { className: "notice" },
                  h("summary", null, `${task.warnings.length} 个扫描问题`),
                  h("pre", null, task.warnings.join("\n")),
                ),
            ),
      ),
    phase === 1 &&
      h(
        "div",
        { className: "permission-layout" },
        h(Permissions, {
          task,
          busy,
          presets: permissionPresets,
          tiers: TIERS,
          onDirty: setPermissionDirty,
          onSave: (permissions) => act("permissions", { permissions }),
        }),
        h(
          "div",
          { className: "permission-graph" },
          h("h3", null, "目录预览与复用"),
          h("p", null, "在目录节点参数中设置整体保护或复用容器；需要详细查看时可全屏。"),
          h(Graph, { ...graphProps, compact: true }),
        ),
      ),
    phase === 2 &&
      (task.mode === "rename"
        ? h(RenameScope, {
            task,
            busy,
            formats,
            onSave: (extensions, web_search) =>
              act("rename_scope", { extensions, web_search }),
          })
        : h(
            React.Fragment,
            null,
            h(
              "div",
              { className: "tab-row" },
              h(
                "div",
                { className: "tabs" },
                h(
                  "button",
                  {
                    className: tab === "target" ? "active" : "",
                    onClick: () => setTab("target"),
                  },
                  "目标目录结构",
                ),
                h(
                  "button",
                  {
                    className: tab === "actual" ? "active" : "",
                    onClick: () => setTab("actual"),
                  },
                  "实际目录与保护",
                ),
              ),
              h("span", null, "更改自动保存 · 未连接节点不参与整理"),
            ),
            task.proposal?.scene ===
              (tab === "target" ? "tree" : "directories") &&
              h(
                "div",
                { className: "notice" },
                h(
                  "p",
                  null,
                  `AI 已生成 ${task.proposal.changes.length} 项建议，审查合并后更新${tab === "target" ? "目标目录树" : "目录类型"}。`,
                ),
                h(
                  Button,
                  { kind: "primary", disabled: busy, onClick: () => ask() },
                  "审查并合并 AI 建议",
                ),
              ),
            tab === "target" &&
              h(InspectionProgress, { inspection: task.inspection }),
            tab === "target"
              ? h(Graph, {
                  task,
                  type: "target",
                  editable: !busy,
                  onNodes,
                  onExamples: setExamples,
                })
              : h(Graph, { ...graphProps }),
            task.proposal?.scene ===
              (tab === "target" ? "tree" : "directories") &&
              h(ProposalView, {
                proposal: task.proposal,
                nodes: task.nodes,
                scene: tab === "target" ? "tree" : "directories",
                busy,
                onApply: (ids) => onMerge(task.proposal, ids),
                onDismiss: () => act("dismiss_proposal"),
              }),
          )),
    phase === 3 &&
      h(
        "div",
        {
          className:
            "planning-options" +
            (task.mode === "rename" ? " rename-planning" : ""),
        },
        ["organize", "desktop"].includes(task.mode) &&
          h(
            "section",
            { className: "plan-option" },
            h(
              "span",
              { className: "option-icon" },
              h(Icon, { name: planReady ? "check" : "tree", size: 27 }),
            ),
            h(
              "h3",
              null,
              planReady
                ? "基础规则计划已就绪"
                : job?.kind === "plan_rules"
                  ? "正在生成基础规则计划"
                  : "基础规则计划尚未就绪",
            ),
            h(
              "p",
              null,
              "进入此阶段时自动按扩展名归类，保留整体目录与已复用的结构。",
            ),
            h(Badge, { kind: planReady ? "mint" : "" }, "自动生成 · 0 token"),
            planReady &&
              h(
                "p",
                { className: "plan-ready-summary", role: "status" },
                `${task.operations.length} 项计划操作 · ${task.retained.length} 项保持原位。${(task.plan_source === "ai" && task.classification?.total > 0) ? "当前计划已包含 AI 细化结果。" : "可以继续 AI 细化，也可以直接进入审查。"}`,
              ),
            !planReady &&
              !busy &&
              h(
                Button,
                {
                  icon: "refresh",
                  onClick: () => act("plan_rules"),
                },
                "重新生成基础计划",
              ),
          ),
        h(
          "section",
          { className: "plan-option ai-option" },
          h("span", { className: "option-icon" }, "AI"),
          h(
            "h3",
            null,
            task.mode === "rename" ? "生成可读文件名" : "继续用 AI 细化",
          ),
          h(
            "p",
            null,
            task.mode === "rename"
              ? "只处理已选择的格式，按权限读取文件，证据不足时保留原名。"
              : "按批把文件分入已有节点。文件名足够时直接分类；需要内容时按权限读取证据。批次随请求大小自动缩小，完成后再审查。",
          ),
          h(
            Badge,
            { kind: "mint" },
            task.mode === "rename"
              ? "按实际 API 用量计费"
              : "可选 · 按实际 API 用量计费",
          ),
          ["organize", "desktop"].includes(task.mode) && classificationControls,
          ["organize", "desktop"].includes(task.mode) && readinessView,
          h(
            Button,
            {
              kind: "primary",
              disabled:
                busy ||
                (["organize", "desktop"].includes(task.mode) && (!planReady || invalidBatchSize || noAiFiles)),
              icon: "spark",
              onClick: () =>
                task.mode === "rename" ? act("rename") : refinePlan(),
            },
            task.mode === "rename"
              ? (["paused", "failed"].includes(task.rename_checkpoint?.status) ? "继续重命名规划" : "生成重命名计划")
              : ["paused", "failed"].includes(task.classification?.status)
                ? "继续批量分类"
                : (task.plan_source === "ai" && task.classification?.total > 0)
                  ? "重新批量分类"
                  : "用 AI 批量分类",
          ),
        ),
      ),
    (phase === 3 || phase === 4) &&
      ["organize", "desktop"].includes(task.mode) &&
      (task.review_classification || task.classification) &&
      h(ClassificationProgress, {
        classification: task.review_classification || task.classification,
        job: job?.task_id === task.id ? job : null,
      }),
    phase === 4 &&
      h(
        React.Fragment,
        null,
        task.mode !== "rename" && h(ReviewEditor, {
          task, busy,
          onSend: async message => {
            await act("review", {selected:[...selected], reviewed:false});
            await act("chat", {scene:"review", message});
          },
          onMerge:(proposal, ids) => onMerge(proposal, ids, [...selected]), onDismiss: () => act("dismiss_proposal"),
        }),
        ["organize", "desktop"].includes(task.mode) &&
          h(
            "details",
            { className: "plan-refinement optional-refinement" },
            h("summary", null, "重新批量分类（可选）"),
            h(
              "div",
              null,
              h(
                "h3",
                null,
                (task.plan_source === "ai" && task.classification?.total > 0)
                  ? "当前计划已由 AI 细化"
                  : "还可以用 AI 细化计划",
              ),
              h(
                "p",
                null,
                "按批分入已有节点；需要内容时使用受权限约束的读取工具。全部批次完成后更新下方计划，目录树保持不变。",
              ),
              classificationControls,
              readinessView,
            ),
            h(
              Button,
              {
                icon: "spark",
                disabled: busy || invalidBatchSize || noAiFiles,
                onClick: refinePlan,
              },
              ["paused", "failed"].includes(task.classification?.status)
                ? "继续批量分类"
                : "用 AI 继续细化",
            ),
          ),
        h(Stats, {
          items: [
            [selected.size, "项待移动 / 重命名"],
            [task.retained.length, "项保持原位"],
            [
              task.operations.filter((o) => o.kind === "directory").length,
              "项整体目录操作",
            ],
            ["0", "项自动删除"],
          ],
        }),
        h(
          "div",
          { className: "comparison" },
          h(
            "section",
            null,
            h(
              "h3",
              null,
              h("span", { className: "compare-dot before" }),
              "整理前",
              h("small", null, "扫描快照"),
            ),
            h(Graph, { task, type: "actual", compact: true }),
          ),
          h(
            "section",
            null,
            h(
              "h3",
              null,
              h("span", { className: "compare-dot after" }),
              "整理后",
              h("small", null, "所选操作的预览"),
            ),
            h(Graph, {
              task,
              type: "actual",
              entries: projectedEntries(task, selected),
              compact: true,
            }),
          ),
        ),
        h(OperationTable, {
          task,
          selected,
          setSelected: (s) => {
            setSelected(s);
            setReviewed(false);
          },
          editable: !busy,
        }),
        h(
          "label",
          { className: "review-check" },
          h("input", {
            type: "checkbox",
            checked: reviewed,
            disabled: busy,
            onChange: (e) => setReviewed(e.target.checked),
          }),
          h(
            "span",
            null,
            "我已审查所选操作及目标位置，确认此计划可以进入执行阶段。",
          ),
        ),
      ),
    phase === 5 &&
      h(
        React.Fragment,
        null,
        h(
          "section",
          { className: "execute-card" },
          h(
            "div",
            { className: "execute-icon" },
            h(Icon, { name: locked ? "check" : "shield", size: 34 }),
          ),
          h(
            "div",
            null,
            h(
              "h3",
              null,
              locked
                ? STATUS[task.status]
                : `即将执行 ${task.operations.filter((o) => o.selected).length} 项操作`,
            ),
            h(
              "p",
              null,
              locked
                ? "实际结果以每项操作状态为准。空目录保留，不进行自动清理。"
                : "目标已固定。执行时如果发现同名冲突或文件改变，会停止并显示原因。",
            ),
          ),
          !locked &&
            h(
              Button,
              {
                kind: "primary",
                icon: "play",
                disabled: busy,
                onClick: () => act("execute"),
              },
              "确认并执行整理",
            ),
          locked &&
            task.status !== "rolled_back" &&
            h(
              Button,
              {
                disabled: busy || task.recycled?.some(r => r.status !== "restored"),
                icon: "refresh",
                onClick: () =>
                  setConfirm({
                    title: "恢复本次已完成的移动？",
                    body: "将按相反顺序，使用每项操作保存的指纹规则核对并恢复文件。检测到变化或原路径被占用时停止，不覆盖任何文件。" +
                      (hasSampledFingerprints(task) ? "本次包含大文件抽样指纹：若未抽样区域被修改且修改时间保持不变，可能无法发现。" : ""),
                    run: () => act("rollback"),
                  }),
              },
              "恢复本次操作",
            ),
        ),
        h(
          "p",
          { className: "fingerprint-note" },
          !locked
            ? fingerprintPolicy === "blake3-adaptive-v2"
              ? "指纹规则：不超过 1 MiB 全量校验；超过 1 MiB 在首尾及中间三处各读取 64 KiB，共 320 KiB，并核对文件大小与修改时间。目录按内部每个文件的大小分别处理。抽样可能遗漏未覆盖区域的修改。"
              : fingerprintPolicy === "blake3-adaptive-v1"
                ? "当前服务仍使用 16 MiB 抽样门槛；关闭旧服务并启动新版后，1 MiB 门槛才会生效。"
                : "当前服务仍使用全量指纹。关闭旧服务并启动新版后，大文件分段抽样规则才会生效。"
            : hasSampledFingerprints(task)
              ? "本次包含分段指纹，具体抽样门槛见各项操作的标签。抽样读取 320 KiB，并核对大小与修改时间；未覆盖区域的修改可能漏检。恢复沿用每项操作保存的规则。"
              : "本次操作保存的是全量内容指纹，恢复时仍需完整读取文件。新的抽样规则不会改变已有恢复记录。",
        ),
        h(OperationTable, {
          task,
          selected: new Set(
            task.operations.filter((o) => o.selected).map((o) => o.id),
          ),
          editable: false,
        }),
        task.recycled?.some(r => r.status !== "restored") && h("p", {className:"notice"}, "有尚未撤销的回收记录，请先在下方撤销回收，再恢复整理。"),
        (task.status === "completed" || task.cleanup.length > 0 || task.recycled?.length > 0) &&
          (desktop ? h("details", {className: "notice"},
            h("summary", null, "文件删除建议与回收站撤销（需手动选择）"),
            h(CleanupPanel, {task, busy, act, recycleSupported})) : h(CleanupPanel, {task, busy, act, recycleSupported})),
      ),
    h(
      "footer",
      { className: "stage-footer" },
      h(
        "div",
        null,
        h(Icon, { name: "shield", size: 16 }),
        phase < 5 ? "当前阶段不会移动文件" : "操作记录保存在本地 JSON / JSONL",
      ),
      h(
        "div",
        null,
        phase > 0 &&
          !locked &&
          h(
            Button,
            { disabled: busy, icon: "back", onClick: () => back(phase - 1) },
            "上一步",
          ),
        phase === 0 &&
          task.scanned &&
          h(
            Button,
            { disabled: busy, icon: "refresh", onClick: () => act("scan") },
            "重新扫描",
          ),
        (phase !== 3 || ["organize", "desktop"].includes(task.mode) || planReady) &&
          phase < 5 &&
          h(
            Button,
            {
              kind: "primary",
              icon: "arrow",
              disabled:
                busy ||
                (phase === 0 && !task.scanned) ||
                (phase === 3 && !planReady) ||
                (phase === 4 && !reviewed) ||
                (phase === 1 && permissionDirty) ||
                (phase === 2 &&
                  task.mode === "rename" &&
                  !task.rename_extensions.length),
              onClick: next,
            },
            "下一步 · " + steps[phase + 1][0],
          ),
      ),
    ),
    examples &&
      h(ExamplePicker, {
        task,
        nodeId: examples,
        onClose: () => setExamples(null),
        onSave: (values) => {
          onNodes(
            task.nodes.map((n) =>
              n.id === examples ? { ...n, examples: values } : n,
            ),
          );
          setExamples(null);
        },
      }),
    logs &&
      h(
        Modal,
        { title: "任务轨迹", wide: true, onClose: () => setLogs(null) },
        h(
          "div",
          { className: "timeline" },
          logs.length
            ? logs.map((e, i) =>
                h(
                  "div",
                  { key: i },
                  h("time", null, new Date(e.timestamp).toLocaleTimeString()),
                  h("b", null, e.kind),
                  h("pre", null, JSON.stringify(e.detail, null, 2)),
                ),
              )
            : h("p", null, "暂无事件"),
        ),
      ),
    confirm &&
      h(
        Modal,
        { title: confirm.title, onClose: () => setConfirm(null) },
        h("p", { className: "modal-description" }, confirm.body),
        h(
          "div",
          { className: "modal-actions" },
          h(Button, { onClick: () => setConfirm(null) }, "取消"),
          h(
            Button,
            {
              kind: "primary",
              onClick: () => {
                confirm.run();
                setConfirm(null);
              },
            },
            "确认",
          ),
        ),
      ),
  );
}

function Stats({ items }) {
  return h(
    "div",
    { className: "stats-grid" },
    ...items.map(([n, l]) =>
      h("div", { key: l }, h("strong", null, n), h("span", null, l)),
    ),
  );
}
function RenameScope({ task, formats, busy, onSave }) {
  const selected = new Set(task.rename_extensions);
  return h(
    "div",
    { className: "scope-panel" },
    h(
      "div",
      { className: "scope-heading" },
      h("h3", null, "选择文件格式"),
      h("small", null, "整体保护目录内的文件自动排除"),
    ),
    h(
      "label",
      { className: "check-label search-opt-in" },
      h("input", {
        type: "checkbox",
        checked: !!task.rename_web_search,
        disabled: busy,
        onChange: (e) => onSave(task.rename_extensions, e.target.checked),
      }),
      "联网检索辅助命名：将已获文件名权限的名称发给设置中的搜索服务。搜索服务单独计费。",
    ),
    h(
      "div",
      { className: "format-grid" },
      ...formats.map((f) =>
        h(
          "button",
          {
            key: f.ext,
            disabled: busy,
            className: "format-card " + (selected.has(f.ext) ? "selected" : ""),
            onClick: () => {
              if (selected.has(f.ext)) selected.delete(f.ext);
              else selected.add(f.ext);
              onSave([...selected]);
            },
          },
          h(Icon, { name: "file" }),
          h("b", null, f.ext ? "." + f.ext : "无扩展名"),
          h("span", null, f.count + " 个文件"),
          h("small", null, sizeLabel(f.size)),
          selected.has(f.ext) && h(Icon, { name: "check" }),
        ),
      ),
    ),
  );
}
function OperationTable({ task, selected, setSelected, editable }) {
  const [filter, setFilter] = useState(""),
    [page, setPage] = useState(0),
    [expanded, setExpanded] = useState(false);
  const tableRef = useRef(null);
  const goPage = next => {
    setPage(next);
    tableRef.current?.scrollIntoView({behavior:"smooth", block:"start"});
  };
  const ops = task.operations.filter((o) =>
    (o.source + o.destination).toLowerCase().includes(filter.toLowerCase()),
  );
  const pages = Math.max(1, Math.ceil(ops.length / 20));
  const currentPage = Math.min(page, pages - 1);
  const pageOps = ops.slice(currentPage * 20, (currentPage + 1) * 20);
  useEffect(() => { setPage(currentPage); }, [currentPage]);
  useEffect(() => { setFilter(""); setPage(0); setExpanded(false); }, [task.id]);
  const labels = {
    pending: "待执行",
    moving: "记录恢复中",
    done: "已完成",
    restoring: "恢复中",
    restored: "已恢复",
    failed: "失败",
  };
  return h(
    "details",
    { className: "operations", ref: tableRef, open: expanded, onToggle: e => setExpanded(e.currentTarget.open) },
    h(
      "summary",
      { className: "operations-summary" },
      h(
        "h3",
        null,
        "具体操作清单",
        h("small", null, task.operations.length + " 项"),
      ),
      h("span", {className:"operations-toggle"}, expanded ? "收起清单" : "展开清单 · 每页 20 项"),
    ),
    expanded && h(React.Fragment, null,
    h("div", {className:"table-heading"},
      h("span", null, `共 ${ops.length} 项${filter ? "匹配" : "操作"} · 已选 ${task.operations.filter(o => selected.has(o.id)).length} 项`),
      h("input", {
        placeholder: "搜索文件或路径…",
        "aria-label": "搜索操作",
        value: filter,
        onChange: (e) => {
          setFilter(e.target.value);
          setPage(0);
        },
      }),
    ),
    h(
      "div",
      { className: "table-scroll" },
      h(
        "table",
        null,
        h(
          "thead",
          null,
          h(
            "tr",
            null,
            editable &&
              h(
                "th",
                null,
                h("input", {
                  type: "checkbox",
                  "aria-label": "全选本页操作",
                  checked:
                    pageOps.length > 0 && pageOps.every((o) => selected.has(o.id)),
                  disabled: !pageOps.length,
                  onChange: (e) =>
                    setSelected(
                      e.target.checked
                        ? new Set([...selected, ...pageOps.map((o) => o.id)])
                        : new Set(
                            [...selected].filter(
                              (id) => !pageOps.some((o) => o.id === id),
                            ),
                          ),
                    ),
                }),
              ),
            h("th", null, "当前位置"),
            h("th", null, "整理后位置"),
            h("th", null, "依据 / 结果"),
          ),
        ),
        h(
          "tbody",
          null,
          ...pageOps.map((o) =>
            h(
              "tr",
              { key: o.id, className: !selected.has(o.id) ? "excluded" : "" },
              editable &&
                h(
                  "td",
                  null,
                  h("input", {
                    type: "checkbox",
                    "aria-label": "选择 " + o.source,
                    checked: selected.has(o.id),
                    onChange: (e) => {
                      const next = new Set(selected);
                      e.target.checked ? next.add(o.id) : next.delete(o.id);
                      setSelected(next);
                    },
                  }),
                ),
              h(
                "td",
                { className: "source-path" },
                o.kind === "directory" && h(Icon, { name: "folder", size: 14 }),
                o.source,
              ),
              h("td", { className: "destination-path" }, o.destination),
              h(
                "td",
                null,
                h(
                  "span",
                  { className: "op-status " + o.status },
                  editable ? o.reason : labels[o.status],
                ),
                !editable && o.fingerprint && h("small", { className: "fingerprint-label" }, fingerprintLabel(o.fingerprint)),
                o.error && h("small", { className: "warning" }, o.error),
              ),
            ),
          ),
          !ops.length &&
            h(
              "tr",
              null,
              h(
                "td",
                { colSpan: editable ? 4 : 3, className: "table-empty" },
                filter ? "没有匹配的操作，请调整搜索条件。" : "没有需要移动的文件。已在正确位置或没有匹配规则的文件保留原位。",
              ),
            ),
        ),
      ),
    ),
    h("nav", {className:"operation-pagination", "aria-label":"操作清单分页"},
      h("span", {role:"status"}, `第 ${currentPage + 1} / ${pages} 页 · ${ops.length ? currentPage * 20 + 1 : 0}–${Math.min((currentPage + 1) * 20, ops.length)} / ${ops.length} 项`),
      h("div", null,
        h(Button, {disabled:currentPage === 0, onClick:()=>goPage(currentPage - 1), "aria-label":"操作清单上一页"}, "上一页"),
        h(Button, {disabled:currentPage === pages - 1, onClick:()=>goPage(currentPage + 1), "aria-label":"操作清单下一页"}, "下一页"))),
    task.retained.length > 0 &&
      h(
        "details",
        { className: "retained" },
        h("summary", null, `${task.retained.length} 项保持原位 · 查看原因`),
        ...task.retained
          .slice(0, 100)
          .map((e, i) =>
            h(
              "div",
              { key: i },
              h("span", null, e.source),
              h("small", null, e.reason),
            ),
          ),
      ),
    ),
  );
}
function ExamplePicker({ task, nodeId, onClose, onSave }) {
  const node = task.nodes.find((n) => n.id === nodeId),
    [search, setSearch] = useState(""),
    [selected, setSelected] = useState(new Set(node.examples));
  const files = task.entries.filter(
    (e) =>
      e.kind === "file" && e.id.toLowerCase().includes(search.toLowerCase()),
  );
  return h(
    Modal,
    { title: "为「" + node.name + "」选择文件示例", onClose },
    h(
      "p",
      { className: "modal-description" },
      "引用本次扫描中的实际文件，发送给 AI 的内容始终服从当前权限规则。",
    ),
    h("input", {
      className: "search-full",
      autoFocus: true,
      placeholder: "按名称或路径搜索…",
      value: search,
      onChange: (e) => setSearch(e.target.value),
    }),
    h(
      "div",
      { className: "example-picker" },
      ...files.slice(0, 200).map((f) =>
        h(
          "label",
          { key: f.id },
          h("input", {
            type: "checkbox",
            checked: selected.has(f.id),
            onChange: (e) =>
              setSelected((s) => {
                const next = new Set(s);
                if (e.target.checked) next.add(f.id);
                else next.delete(f.id);
                return next;
              }),
          }),
          h(Icon, { name: "file" }),
          h("span", null, f.id),
          h("small", null, sizeLabel(f.size)),
        ),
      ),
    ),
    h(
      "div",
      { className: "modal-actions" },
      h("span", null, selected.size + " 个文件已选"),
      h(
        Button,
        { kind: "primary", onClick: () => onSave([...selected]) },
        "使用这些文件",
      ),
    ),
  );
}

function describeChange(change, side, names) {
  const value = change[side];
  if (value == null) return "";
  if (change.kind === "placement") return value.node_id ? "目标目录：" + (names.get(value.node_id) || value.node_id) : "保持原位";
  if (change.kind === "directory") return classLabel[value] || value;
  if (change.kind !== "node") return JSON.stringify(value, null, 2);
  const labels = {
    name: "文件夹名",
    parent: "位于",
    rule_type: "规则类型",
    extensions: "扩展名",
    note: "分类描述 / 备注",
    examples: "文件示例",
    mapping: "复用目录",
  };
  const other = change[side === "before" ? "after" : "before"];
  return Object.keys(labels)
    .flatMap((key) => {
      const current = value[key];
      if (other && JSON.stringify(current) === JSON.stringify(other[key]))
        return [];
      if (
        !other &&
        (current == null ||
          current === "" ||
          (Array.isArray(current) && !current.length))
      )
        return [];
      if (key === "extensions" || key === "examples") {
        const normalize = (v) =>
          key === "extensions" ? String(v).toLowerCase().replace(/^\./, "") : v;
        const opposite = new Set((other?.[key] || []).map(normalize));
        const changed = [...new Set((current || []).map(normalize))].filter(
          (v) => !opposite.has(v),
        );
        return changed.length ? [labels[key] + "：" + changed.join(", ")] : [];
      }
      const text =
        key === "parent"
          ? current === "root"
            ? "整理根目录"
            : names.get(current) || current || "未连接"
          : key === "rule_type"
            ? { simple: "扩展名分类", complex: "语义分类" }[current] || current
            : (current ?? "未设置");
      return [labels[key] + "：" + (text || "未设置")];
    })
    .join("\n");
}
function ClassificationControls({
  batchSize,
  setBatchSize,
  thinking,
  setThinking,
  disabled,
}) {
  return h(
    "div",
    { className: "classification-controls" },
    h(
      "label",
      null,
      "每批文件数上限",
      h("input", {
        type: "number",
        min: 1,
        max: 1024,
        step: 1,
        value: batchSize,
        disabled,
        onChange: (e) => setBatchSize(e.target.value),
      }),
    ),
    h(
      "label",
      { className: "classification-thinking" },
      h("input", {
        type: "checkbox",
        checked: thinking,
        disabled,
        onChange: (e) => setThinking(e.target.checked),
      }),
      "分类时启用思考（较慢）",
    ),
    h(
      "small",
      null,
      "实际批量由请求体积、内容证据和输出预算决定；调整选项会重新分类。",
    ),
  );
}

function ParallelProgress({run, compact = false}) {
  const {active, done, queued} = parallelSummary(run);
  return h("div", {className:"parallel-progress" + (compact ? " compact" : ""), "aria-label":"并行任务状态"},
    h("div", {className:"parallel-counts"},
      h("span", null, `进行中 ${active.length}${run.limit ? " / " + run.limit + " 路" : " 批"}`),
      h("span", null, `排队 ${queued.length} 批`),
      h("span", null, `完成 ${done.length} 批`)),
    h("div", {className:"parallel-lanes"}, ...active.slice(0, compact ? 4 : 12).map(b =>
      h("span", {key:b.id, className:"parallel-lane", title:`${b.branch} · ${b.files} 个文件`},
        h("i", {className:"spinner"}), `批次 ${run.batches.findIndex(item=>item.id===b.id)+1} · ${b.branch} · ${b.files} 个`)),
      active.length > (compact ? 4 : 12) && h("span", null, `另有 ${active.length - (compact ? 4 : 12)} 批进行中`)),
    !compact && h("small", null, "百分比只计入已完成的文件；进行中的批次不计为完成。"));
}

function ReviewEditor({task, busy, onSend, onMerge, onDismiss}) {
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [sending, setSending] = useState(false);
  async function submit(event) {
    event.preventDefault();
    if (!message.trim() || busy || sending) return;
    setSending(true); setError("");
    try { await onSend(message.trim()); setMessage(""); } catch (e) { setError(e.message); }
    finally { setSending(false); }
  }
  return h("section", {className:"review-editor"},
    h("h3", null, "告诉 AI，哪里还需要调整"),
    h("p", null, "可以调整目标目录和文件归属。采纳后自动完成位置规划，并更新下方‘整理后’预览；需要按内容细分时，仅对受影响的文件调用 AI。文件仍在最终执行步骤移动。"),
    h("form", {onSubmit:submit},
      h("textarea", {value:message, onChange:e=>setMessage(e.target.value), disabled:busy || sending, rows:2, maxLength:16000,
        "aria-label":"描述整理计划需要调整的地方", placeholder:'例如：把视频从“其它”移出来，放进新的“视频”一级目录，其他分类保留。'}),
      h(Button, {type:"submit", kind:"primary", icon:"spark", disabled:busy || sending || !message.trim()}, "生成修改建议")),
    error && h("p", {role:"alert"}, error),
    task.proposal?.scene === "review" && h(ProposalView, {proposal:task.proposal, nodes:task.nodes, scene:"review", busy,
      onApply:ids=>onMerge(task.proposal,ids), onDismiss}),
  );
}

function ClassificationProgress({ classification: run, job }) {
  const classifying = ["plan_ai", "review_proposal"].includes(job?.kind);
  // Older versions recorded a zero-batch run as a successful AI plan.
  if (run.total === 0 && !classifying)
    return h("section", {className:"inspection-card classification-progress"},
      h("strong", null, "上次未调用 AI"),
      h("p", null, "没有符合语义分类条件的文件，保留了基础规则计划。请查看上方原因并补充目标规则或调整权限。"));
  if (classifying && !job.parallel && run.status !== "running")
    return h(
      "section",
      {
        className: "inspection-card classification-progress",
        "aria-label": "批量分类进度",
      },
      h(
        "header",
        null,
        h("strong", null, "正在批量分类"),
        h(
          "span",
          null,
          job.total ? `${job.current} / ${job.total} 个文件` : "正在组织批次",
        ),
      ),
      h("progress", {
        max: Math.max(job.total, 1),
        value: job.current,
        "aria-label": "分类完成进度",
      }),
      h("p", null, "正在处理当前分类任务，已有计划保留到全部批次完成。"),
    );
  if (classifying && job.parallel) run = {...run, status:"running", batches:job.parallel.batches, completed:job.parallel.completed_files, total:job.parallel.total_files};
  if (run.status === "running" && !classifying) run = {...run, status:"paused", batches:run.batches.map(b=>b.status === "running" ? {...b,status:"pending"} : b)};
  const completed = run.batches.filter((b) => b.status === "complete").length;
  const labels = {
    running: "正在批量分类",
    complete: "批量分类已完成",
    paused: "批量分类已暂停",
    failed: "批量分类中断，可继续",
  };
  return h(
    "section",
    {
      className: "inspection-card classification-progress",
      "aria-label": "批量分类进度",
    },
    h(
      "header",
      null,
      h("strong", null, labels[run.status] || "批量分类"),
      h(
        "span",
        null,
        `${run.completed} / ${run.total} 个文件 · ${completed} / ${run.batches.length} 批`,
      ),
    ),
    h("progress", {
      max: Math.max(run.total, 1),
      value: run.status === "complete" ? Math.max(run.total, 1) : run.completed,
      "aria-label": "分类完成进度",
    }),
    h(
      "p",
      null,
      run.status === "complete"
        ? "分类结果已写入待审查计划。"
        : "完成的批次已保存。全部分类完成后再更新计划，当前已有计划仍保留。",
    ),
    h(
      "small",
      null,
      `${run.skipped} 个无需语义细化、受权限限制或保持原位的文件未交给 AI · ${run.protected_files} 个整体保护目录内文件未展开`,
    ),
    run.error &&
      h("p", { className: "classification-error", role: "alert" }, run.error),
    h(ParallelProgress, {run:job?.parallel || run}),
    h(
      "details",
      null,
      h("summary", null, `查看 ${run.batches.length} 个批次`),
      ...run.batches.map((b, i) =>
        h(
          "article",
          { key: b.id },
          h("strong", null, `${i + 1}. ${b.branch}`),
          h(
            "span",
            null,
            `${b.files} 个文件 · ${{ pending: "等待分类", running: "处理中", complete: "已完成" }[b.status] || b.status}`,
          ),
          b.status === "complete" &&
            h(
              "p",
              null,
              b.decisions ? `${b.decisions.filter((d) => d.node_id !== null).length} 个已确定目录 · ${b.decisions.filter((d) => d.node_id === null).length} 个证据不足，保留原计划目标` : "分类结果已保存",
            ),
        ),
      ),
    ),
  );
}

function InspectionProgress({ inspection }) {
  if (!inspection)
    return h(
      "div",
      { className: "inspection-intro" },
      "AI 先看类型总览，再按类型分批检查；每批最多 24 个文件，只把各批摘要用于目录建议。",
    );
  const total = inspection.groups.reduce((n, g) => n + g.eligible, 0);
  const done = inspection.groups.reduce((n, g) => n + g.inspected, 0);
  const withheld = inspection.groups.reduce((n, g) => n + g.withheld, 0);
  const labels = {
    running: "正在分类型检查",
    complete: "类型检查已完成",
    paused: "检查已暂停",
    failed: "检查中断，可继续",
  };
  return h(
    "section",
    { className: "inspection-card", "aria-label": "AI 文件检查进度" },
    h(
      "header",
      null,
      h("strong", null, labels[inspection.status] || "分类型检查"),
      h("span", null, `${done} / ${total} 个可读文件`),
    ),
    h("progress", {
      max: Math.max(total, 1),
      value: inspection.status === "complete" ? Math.max(total, 1) : done,
      "aria-label": "文件检查完成进度",
    }),
    inspection.overview && h("p", null, inspection.overview),
    h(
      "small",
      null,
      `每批最多 ${inspection.batch_size} 个文件 · ${withheld} 个受权限限制或尚在下载的文件未读取 · ${inspection.protected_files} 个整体保护目录内文件未展开`,
    ),
    ["paused", "failed"].includes(inspection.status) &&
      h("p", null, "已保存完成批次，继续时会从未完成的位置接着检查。"),
    h(
      "details",
      null,
      h("summary", null, `查看 ${inspection.groups.length} 类文件的检查结果`),
      ...inspection.groups.map((g) =>
        h(
          "article",
          { key: g.id },
          h("strong", null, g.label),
          h(
            "span",
            null,
            `${g.inspected} / ${g.eligible} 个 · ${g.batches} 批`,
          ),
          h(
            "p",
            null,
            g.summary ||
              (g.eligible
                ? "等待检查"
                : "此类型仅保留数量统计，没有可读取文件"),
          ),
        ),
      ),
    ),
  );
}

function ProposalView({
  proposal,
  nodes = [],
  scene,
  busy,
  onApply,
  onDismiss,
}) {
  const [selected, setSelected] = useState(
    new Set(proposal.changes.map((c) => c.id)),
  );
  const [merging, setMerging] = useState(false),
    [mergeError, setMergeError] = useState("");
  const mergePending = useRef(false);
  useEffect(() => {
    setSelected(new Set(proposal.changes.map((c) => c.id)));
    setMergeError("");
  }, [proposal.id]);
  async function merge() {
    if (mergePending.current || busy || !selected.size) return;
    mergePending.current = true;
    setMerging(true);
    setMergeError("");
    try {
      await onApply([...validProposalSelection(selected, proposalDependencies(proposal.changes, nodes))]);
    } catch (error) {
      setMergeError(error.message || "合并失败，请重试");
    } finally {
      mergePending.current = false;
      setMerging(false);
    }
  }
  const names = new Map(nodes.map((n) => [n.id, n.name]));
  for (const c of proposal.changes) {
    if (c.kind === "node")
      names.set(c.target, c.after?.name || c.before?.name || c.label);
  }
  const dependencies = proposalDependencies(proposal.changes, nodes);
  const selectedValid = validProposalSelection(selected, dependencies);
  function selectChange(change, checked) {
    setSelected(previous => {
      const next = new Set(previous);
      if (checked) next.add(change.id);
      else next.delete(change.id);
      return validProposalSelection(next, dependencies);
    });
  }
  return h(
    "section",
    { className: "proposal" },
    h(
      "header",
      null,
      h("span", { className: "ai-word" }, "AI"),
      h("h3", null, "建议改动", h("small", null, "等待你的选择")),
      h(Badge, null, proposal.changes.length + " 项"),
    ),
    h("p", null, proposal.message),
    [...dependencies.values()].some(d => d.size) &&
      h(
        "p",
        { className: "proposal-hint" },
        "取消目录建议后，依赖它的子目录和文件归属建议会取消勾选并置灰。重新采纳目录后，可自行勾选相关建议。",
      ),
    ...proposal.changes.map((c) => {
      const missing = [...(dependencies.get(c.id) || [])].filter(id => !selectedValid.has(id));
      const blocked = missing.length > 0;
      const before = describeChange(c, "before", names);
      const after = describeChange(c, "after", names);
      return h(
        "div",
        { key: c.id, className: "change" + (blocked ? " change-blocked" : "") },
        h(
          "label",
          null,
          h("input", {
            type: "checkbox",
            checked: selectedValid.has(c.id),
            disabled: busy || merging || blocked,
            "aria-label": `采纳建议：${c.after?.name || c.label}`,
            onChange: (e) => selectChange(c, e.target.checked),
          }),
          h("b", null, c.after?.name || c.label),
          h(
            Badge,
            null,
            c.before == null
              ? "新增节点"
              : c.after == null
                ? "删除节点，保留文件"
                : "修改",
          ),
        ),
        blocked && h("p", {className:"proposal-hint"}, "需先采纳：" + missing.map(id => { const c = proposal.changes.find(c => c.id === id); return c?.after?.name || c?.label || id; }).join("、")),
        before &&
          h(
            "pre",
            { className: "diff-removed" },
            before
              .split("\n")
              .map((line) => "− " + line)
              .join("\n"),
          ),
        after &&
          h(
            "pre",
            { className: "diff-added" },
            after
              .split("\n")
              .map((line) => "+ " + line)
              .join("\n"),
          ),
      );
    }),
    mergeError &&
      h(
        "p",
        { className: "proposal-error", role: "alert" },
        "合并失败：" + mergeError,
      ),
    h(
      "footer",
      null,
      h(Button, { disabled: busy || merging, onClick: onDismiss }, "放弃建议"),
      h(Button, { disabled: busy || merging, onClick: () => setSelected(new Set(proposal.changes.map(c => c.id))) }, "全选建议"),
      h(Button, { disabled: busy || merging, onClick: () => setSelected(new Set()) }, "清空勾选"),
      h(
        Button,
        {
          kind: "primary",
          disabled:
            busy || merging || !selectedValid.size || scene !== proposal.scene,
          onClick: merge,
        },
        merging ? "正在应用…" : scene === "review" ? "应用并更新整理后预览（" + selectedValid.size + " 项）" : "合并所选 " + selectedValid.size + " 项",
      ),
    ),
  );
}

function Assistant({
  open,
  setOpen,
  task,
  scene,
  prompt,
  onPromptUsed,
  onSend,
  busy,
  usage,
  budget,
  act,
  onMerge,
}) {
  const [max, setMax] = useState(false),
    [inspectFiles, setInspectFiles] = useState(true),
    [text, setText] = useState(""),
    [position, setPosition] = useState(null),
    [drag, setDrag] = useState(null);
  const body = useRef(null),
    moved = useRef(false);
  useEffect(() => {
    if (prompt) {
      setText(prompt);
      onPromptUsed();
    }
  }, [prompt]);
  useEffect(() => {
    if (body.current) body.current.scrollTop = body.current.scrollHeight;
  }, [task?.messages.length, open]);
  useEffect(() => {
    if (!drag) return;
    const move = (e) => {
      if (
        Math.abs(e.clientX - drag.startX) + Math.abs(e.clientY - drag.startY) >
        5
      )
        moved.current = true;
      setPosition({
        x: Math.max(8, Math.min(innerWidth - drag.w - 8, e.clientX - drag.dx)),
        y: Math.max(
          68,
          Math.min(innerHeight - drag.h - 8, e.clientY - drag.dy),
        ),
      });
    };
    const up = () => setDrag(null);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [drag, open]);
  function startDrag(e) {
    if (max || e.target.closest("button,textarea,input")) return;
    const r = e.currentTarget.parentElement.getBoundingClientRect();
    moved.current = false;
    setDrag({
      dx: e.clientX - r.left,
      dy: e.clientY - r.top,
      w: r.width,
      h: r.height,
      startX: e.clientX,
      startY: e.clientY,
    });
  }
  const messages = task?.messages.filter((m) => m.scene === scene) || [];
  const sceneName = {
    home: "首页",
    settings: "模型设置",
    history: "任务历史",
    scan: "扫描目录",
    permissions: "读取权限",
    tree: "目标结构",
    directories: "实际目录与保护",
    planning: "生成计划",
    review: "审查结果",
    execution: "执行记录",
  }[scene];
  const style =
    position && !max
      ? {
          left: Math.max(
            8,
            Math.min(position.x, innerWidth - (open ? 400 : 58) - 8),
          ),
          top: Math.max(
            68,
            Math.min(position.y, innerHeight - (open ? 570 : 58) - 8),
          ),
          right: "auto",
          bottom: "auto",
        }
      : {};
  if (!open)
    return h(
      "button",
      {
        className: "ai-launcher",
        style,
        onPointerDown: (e) => {
          const r = e.currentTarget.getBoundingClientRect();
          moved.current = false;
          setDrag({
            dx: e.clientX - r.left,
            dy: e.clientY - r.top,
            w: r.width,
            h: r.height,
            startX: e.clientX,
            startY: e.clientY,
          });
        },
        onClick: () => {
          if (!moved.current) setOpen(true);
        },
        "aria-label": "打开 AI 助手",
      },
      "AI",
      h("span", null, "协作助手"),
    );
  return h(
    "aside",
    {
      className: "assistant " + (max ? "maximized" : ""),
      style,
      "aria-label": "AI 协作助手",
    },
    h(
      "header",
      { className: "assistant-header", onPointerDown: startDrag },
      h("span", { className: "ai-word" }, "AI"),
      h(
        "div",
        null,
        h("b", null, "你的整理搭档"),
        h("small", null, "当前场景 · " + sceneName),
      ),
      h(
        "button",
        {
          onClick: () => setMax(!max),
          "aria-label": max ? "还原 AI 窗口" : "最大化 AI 窗口",
        },
        h(Icon, { name: "max", size: 15 }),
      ),
      h(
        "button",
        {
          onClick: () => {
            setOpen(false);
            setMax(false);
          },
          "aria-label": "收起 AI 助手",
        },
        h(Icon, { name: "minus", size: 17 }),
      ),
    ),
    h(
      "div",
      { className: "assistant-context" },
      h("span", { className: "pill-dot" }),
      (["tree", "directories", "permissions"].includes(scene) || (scene === "review" && task?.mode !== "rename"))
        ? "建议只针对当前可见内容，合并后才生效"
        : "当前场景提供解释与建议，不修改文件",
    ),
    task?.chat_context?.scene === scene && h("div", {className:"assistant-memory"},
      `上次请求：${task.chat_context.included}/${task.chat_context.total} 条完整历史` +
      (task.chat_context.excerpted ? ` · ${task.chat_context.excerpted} 条早期摘录` : "") +
      (task.chat_context.omitted ? ` · ${task.chat_context.omitted} 条未携带（完整历史仍保存在本机）` : "")),
    h(
      "div",
      { className: "assistant-body", ref: body },
      !messages.length &&
        h(
          "div",
          { className: "chat-welcome" },
          h("span", null, "一起理清思路。"),
          h(
            "p",
            null,
            "可以让我解释规则、建议分类结构，或帮你判断一个目录是否应该整体保留。",
          ),
          h(
            "button",
            {
              onClick: () =>
                setText(
                  scene === "tree"
                    ? "请根据当前模板和目录统计，给出减少文件移动的结构建议。"
                    : "请解释当前步骤应该如何操作。",
                ),
            },
            scene === "tree"
              ? "建议一个更适合我的目录结构 ↗"
              : "这一步应该怎么做？ ↗",
          ),
        ),
      ...messages.map((m, i) =>
        h(
          "div",
          { key: i, className: "chat-message role-" + m.role },
          h("small", null, m.role === "user" ? "你" : "AI"),
          h("div", null, m.content),
        ),
      ),
      busy &&
        h(
          "div",
          { className: "chat-thinking" },
          h("span", { className: "spinner" }),
          "正在处理，进度显示在页面下方…",
        ),
      task?.proposal?.scene === scene &&
        h(ProposalView, {
          proposal: task.proposal,
          nodes: task.nodes,
          scene,
          busy,
          onApply: (ids) => onMerge(task.proposal, ids),
          onDismiss: () => act("dismiss_proposal"),
        }),
    ),
    scene === "tree" &&
      ["organize", "desktop"].includes(task?.mode) &&
      task.phase === 2 &&
      h(
        "label",
        { className: "assistant-inspect" },
        h("input", {
          type: "checkbox",
          checked: inspectFiles,
          disabled: busy,
          onChange: (e) => setInspectFiles(e.target.checked),
        }),
        "先按类型检查文件（已完成批次会复用）",
      ),
    h(
      "form",
      {
        className: "assistant-input",
        onSubmit: (e) => {
          e.preventDefault();
          if (text.trim() && !busy) {
            onSend(
              text,
              inspectFiles &&
                scene === "tree" &&
                ["organize", "desktop"].includes(task?.mode) &&
                task.phase === 2,
            );
            setText("");
          }
        },
      },
      h("textarea", {
        rows: 2,
        "aria-label": "发给 AI 的消息",
        placeholder: "聊聊你想怎样整理…",
        value: text,
        onChange: (e) => setText(e.target.value),
        onKeyDown: (e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            e.currentTarget.form.requestSubmit();
          }
        },
      }),
      h(
        "button",
        {
          type: "submit",
          disabled: busy || !text.trim(),
          "aria-label": "发送给 AI",
        },
        h(Icon, { name: "arrow" }),
      ),
    ),
    h(
      "footer",
      { className: "assistant-usage" },
      h(
        "span",
        null,
        `输入 ${usage.input.toLocaleString()} / 输出 ${usage.output.toLocaleString()}`,
      ),
      h("b", null, costText(usage.costs)),
      budget &&
        h("small", null, `预算 ${usage.input + usage.output} / ${budget}`),
    ),
  );
}

function Settings({ config, busy, onSave, onTest, onBack, onDiscover, onPricing }) {
  const [form, setForm] = useState({
      ...config,
      llm: { ...config.llm, api_key: "" },
      search: {
        enabled: false,
        endpoint: "https://api.tavily.com/search",
        ...config.search,
        api_key: "",
      },
    }),
    [saved, setSaved] = useState(true),
    [showKey, setShowKey] = useState(false),
    [clearKey, setClearKey] = useState(false),
    [clearSearchKey, setClearSearchKey] = useState(false),
    [catalog, setCatalog] = useState({ providers: [], models: [] }),
    [discovery, setDiscovery] = useState(null),
    [modelLoading, setModelLoading] = useState(false),
    [modelError, setModelError] = useState(""),
    [refreshModels, setRefreshModels] = useState(0),
    [providerOverride, setProviderOverride] = useState(null),
    [pricePreview, setPricePreview] = useState(null),
    [media, setMedia] = useState(null);
  useEffect(()=>{const c=new AbortController();fetch("/api/media-capabilities",{signal:c.signal}).then(r=>r.json()).then(setMedia).catch(()=>{});return ()=>c.abort();},[]);
  const pricingRef = useRef(onPricing);
  pricingRef.current = onPricing;
  useEffect(() => {
    const controller = new AbortController();
    setPricePreview(null);
    const timer = setTimeout(() => pricingRef.current({endpoint:form.llm.endpoint,model:form.llm.model,pricing:form.llm.pricing}, controller.signal).then(setPricePreview).catch(e=>{if(!controller.signal.aborted)setPricePreview({note:e.message,status:"unknown"});}), 200);
    return () => {clearTimeout(timer);controller.abort();};
  }, [form.llm.endpoint,form.llm.model,form.llm.pricing]);
  const discoverRef = useRef(onDiscover);
  discoverRef.current = onDiscover;
  const base = (endpoint) => String(endpoint || "").trim().replace(/\/(chat\/completions|responses|messages)\/?$/, "").replace(/\/$/, "");
  const canReuseKey = base(form.llm.endpoint) === base(config.llm.endpoint) && !clearKey;
  const providerId = providerOverride || catalog.providers.find((p) => {
    try { return new URL(p.endpoint).host === new URL(form.llm.endpoint).host; } catch { return false; }
  })?.id || "custom";
  useEffect(() => {
    const controller = new AbortController();
    fetch("/api/model-presets", { signal: controller.signal }).then((r) => r.json()).then(setCatalog).catch(() => {});
    return () => controller.abort();
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    setDiscovery(null);
    setModelError("");
    setModelLoading(false);
    let url;
    try { url = new URL(form.llm.endpoint); } catch { return () => controller.abort(); }
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (!form.llm.api_key.trim() && !(canReuseKey && config.has_api_key) && !local) {
      setModelError("填写当前服务商的密钥后自动查询。下方官方预设尚未验证账号权限。");
      return () => controller.abort();
    }
    const timer = setTimeout(async () => {
      setModelLoading(true);
      try {
        const result = await discoverRef.current({
          endpoint: form.llm.endpoint, api_key: form.llm.api_key,
          api_format: form.llm.api_format || "auto",
          use_saved_key: canReuseKey && !form.llm.api_key.trim(),
        }, controller.signal);
        if (!controller.signal.aborted) setDiscovery(result);
      } catch (error) {
        if (!controller.signal.aborted) setModelError(error.message);
      } finally {
        if (!controller.signal.aborted) setModelLoading(false);
      }
    }, 700);
    return () => { clearTimeout(timer); controller.abort(); };
  }, [form.llm.endpoint, form.llm.api_key, form.llm.api_format, config.has_api_key, config.llm.endpoint, clearKey, refreshModels]);
  const presets = (discovery?.presets || catalog.models.filter((m) => m.provider === providerId)).map((m) => ({
    ...m, name: m.name || m.id, available: false,
    context_source: m.context_length ? "preset" : "unknown",
    output_source: m.max_output_tokens ? "preset" : "unknown", source_url: m.source_url || m.source,
  }));
  const availableModels = discovery?.status === "ok" ? discovery.models : [];
  const otherPresets = presets.filter((p) => !availableModels.some((m) => m.id === p.id));
  const modelChoices = [...availableModels, ...otherPresets];
  const selectedModel = modelChoices.find((m) => m.id === form.llm.model);
  const sourceLabel = (source) => ({ api: "接口返回", preset: "官方预设", unknown: "未知" }[source] || "未知");
  const chooseModel = (id) => {
    const info = modelChoices.find((m) => m.id === id);
    setForm((f) => ({ ...f, llm: {
      ...f.llm, model: id,
      context_length: info?.context_length || f.llm.context_length,
      max_output_tokens: Math.min(f.llm.max_output_tokens || 16384, info?.max_output_tokens || 4000000, info?.context_length || 4000000),
      multimodal: info?.vision ?? false,
    } }));
    setSaved(false);
  };
  const chooseProvider = (id) => {
    setProviderOverride(id);
    const provider = catalog.providers.find((p) => p.id === id);
    if (!provider) return;
    setForm((f) => ({ ...f, llm: { ...f.llm, endpoint: provider.endpoint,
      api_format: provider.api_format, api_key: "", model: "", multimodal: false } }));
    setClearKey(false);
    setSaved(false);
  };
  const llm = (key, value) => {
    if (key === "endpoint") setProviderOverride(null);
    setForm((f) => ({ ...f, llm: { ...f.llm, [key]: value,
      ...(key === "endpoint" ? { api_key: "" } : {}),
      ...(key === "model" ? { multimodal: false } : {}),
    } }));
    setSaved(false);
  };
  const set = (key, value) => {
    setForm((f) => ({ ...f, [key]: value }));
    setSaved(false);
  };
  const search = (key, value) => {
    setForm((f) => ({ ...f, search: { ...f.search, [key]: value } }));
    setSaved(false);
  };
  const pricing = (key, value) => {
    setForm(f=>({...f,llm:{...f.llm,pricing:{...f.llm.pricing,[key]:value}}}));
    setSaved(false);
  };
  const price = (key, value) => {
    setForm((f) => ({
      ...f,
      llm: { ...f.llm, pricing: { ...f.llm.pricing, [key]: value == null ? null : value / 1000 } },
    }));
    setSaved(false);
  };
  return h(
    "div",
    { className: "settings-wrap" },
    h(
      "div",
      { className: "page-title" },
      h(
        "div",
        null,
        h("div", { className: "eyebrow" }, "MODEL & PREFERENCES"),
        h("h1", null, "让 AI 按你的方式工作"),
        h("p", null, "选择服务商、填写密钥，再从账号模型列表选择。支持 OpenAI、Responses、Claude 原生接口及本地模型。"),
      ),
      h(Button, { icon: "back", onClick: onBack }, "返回工作台"),
    ),
    h("nav",{className:"settings-nav","aria-label":"设置分组"},
      [["connection","1 连接与模型"],["performance","2 能力与速度"],["pricing","3 费用"],["search","可选检索"],["defaults","默认目录"]].map(([id,label])=>
        h("button",{key:id,type:"button",onClick:()=>{const el=document.getElementById("settings-"+id);if(el?.tagName==="DETAILS")el.open=true;el?.scrollIntoView({behavior:"smooth",block:"start"});}},label))),
    h(
      "form",
      {
        onSubmit: async (e) => {
          e.preventDefault();
          try {
            const updated = await onSave(form, clearKey, clearSearchKey);
            setSaved(true);
            setClearKey(false);
            setClearSearchKey(false);
            setForm((f) => ({
              ...(updated || f),
              llm: { ...(updated?.llm || f.llm), api_key: "" },
              search: { ...(updated?.search || f.search), api_key: "" },
            }));
          } catch {}
        },
      },
      h(
        "section",
        { className: "settings-card", id: "settings-connection" },
        h(
          "div",
          { className: "card-title" },
          h(Icon, { name: "spark" }),
          h("h2", null, "模型连接"),
          h(
            Badge,
            { kind: (form.llm.api_key.trim() || (canReuseKey && config.has_api_key)) ? "mint" : "" },
            form.llm.api_key.trim() ? "新密钥待保存" : (canReuseKey && config.has_api_key) ? "已配置密钥" : "未配置密钥",
          ),
        ),
        h("div", { className: "settings-grid" },
          h(Field, { label: "服务商", hint: "API Key 不能可靠识别服务商，请先确认地址。" },
            h("select", { value: providerId, onChange: (e) => chooseProvider(e.target.value) },
              h("option", { value: "custom" }, "自定义 / 中转 / 本地服务"),
              catalog.providers.map((p) => h("option", { key: p.id, value: p.id }, p.name)))),
          h(Field, { label: "API 协议", hint: "自动模式识别官方端点；中转服务可手动指定。" },
            h("select", { value: form.llm.api_format || "auto", onChange: (e) => llm("api_format", e.target.value) },
              h("option", { value: "auto" }, "自动选择"),
              h("option", { value: "chat_completions" }, "OpenAI Chat Completions"),
              h("option", { value: "responses" }, "OpenAI Responses"),
              h("option", { value: "anthropic" }, "Anthropic Messages")))),
        h(
          Field,
          {
            label: "API Endpoint",
            hint: "接受 API 基础地址或完整请求地址。改变地址后需重新填写密钥；不会自动转发旧密钥。",
          },
          h("input", {
            required: true,
            type: "url",
            value: form.llm.endpoint,
            onChange: (e) => llm("endpoint", e.target.value),
          }),
        ),
        h(
          "div",
          { className: "settings-grid key-only" },
          h(
            Field,
            {
              label: "API Key",
              hint: "输入后自动查询模型，查询不保存密钥。点击保存时仅写入本机私有 .env；新密钥用于本次连接。",
            },
            h(
              "div",
              { className: "key-input" },
              h("input", {
                type: showKey ? "text" : "password",
                autoComplete: "off",
                value: form.llm.api_key,
                placeholder: config.has_api_key && canReuseKey
                  ? "已从环境变量配置 · 可输入新密钥"
                  : "本地无鉴权服务可留空",
                onChange: (e) => llm("api_key", e.target.value),
              }),
              h(
                "button",
                { type: "button", onClick: () => setShowKey(!showKey) },
                showKey ? "隐藏" : "显示",
              ),
            ),
          ),
        ),
        h("div", { className: "model-discovery", "aria-live": "polite" },
          h("div", { className: "connection-test" },
            h("span", null, modelLoading ? "正在查询账号模型…" : (modelError || discovery?.message || "填写密钥后自动获取模型名单；仅查询列表，不发送生成请求。")),
            h(Button, { type: "button", icon: "refresh", disabled: modelLoading,
              onClick: () => setRefreshModels((n) => n + 1) }, "刷新模型")),
          h(Field, { label: "选择模型", hint: `上下文数值来源单独标注。预设核对日期：${catalog.verified_at || "加载中"}。` },
            h("select", { value: selectedModel ? form.llm.model : "", onChange: (e) => e.target.value && chooseModel(e.target.value) },
              h("option", { value: "" }, form.llm.model ? `手动填写：${form.llm.model}` : "请选择模型或手动填写 ID"),
              availableModels.length > 0 && h("optgroup", { label: "账号接口返回" }, availableModels.map((m) =>
                h("option", { key: m.id, value: m.id, disabled: m.task_compatible === false },
                  `${m.id} · ${m.context_length ? m.context_length.toLocaleString() + " token" : "上下文未知"}${m.task_compatible === false ? " · 不适用于整理任务" : ""}`))),
              otherPresets.length > 0 && h("optgroup", { label: "官方预设 · 未验证账号可用性" }, otherPresets.map((m) =>
                h("option", { key: m.id, value: m.id }, `${m.id} · ${m.context_length ? m.context_length.toLocaleString() + " token" : "上下文未知"}`))))),
          h(
            Field,
            { label: "模型 ID（也可手动填写）", hint: "上方列表选择后自动带入已知能力；未知模型保留手动配置。" },
            h("input", {
              required: true,
              value: form.llm.model,
              onChange: (e) => llm("model", e.target.value),
            }),
          ),
          selectedModel && h("div", { className: "model-capabilities" },
            h("p", null, `上下文：${selectedModel.context_length?.toLocaleString() || "未知"}（${sourceLabel(selectedModel.context_source)}） · 最大输出：${selectedModel.max_output_tokens?.toLocaleString() || "未知"}（${sourceLabel(selectedModel.output_source)}）`),
            h("p", null, `图像输入：${selectedModel.vision == null ? "未知" : selectedModel.vision ? "支持" : "不支持"} · 工具调用：${selectedModel.tools == null ? "未知" : selectedModel.tools ? "支持" : "不支持"}。${selectedModel.note || ""}`),
            selectedModel.source_url && h("a", { href: selectedModel.source_url, target: "_blank", rel: "noreferrer" }, "官方能力说明"),
            h(Button, { type: "button", onClick: () => chooseModel(form.llm.model) }, "应用已知能力")),
          !selectedModel && form.llm.model && h("p", { className: "muted" }, "此模型没有已核实的能力数据，请手动确认上下文、输出上限和视觉能力。")),
        h(
          "label",
          { className: "check-label" },
          h("input", {
            type: "checkbox",
            checked: clearKey,
            onChange: (e) => {
              setClearKey(e.target.checked);
              setSaved(false);
            },
          }),
          "解除当前连接的密钥绑定（不会删除 .env 中的其他凭据）",
        ),
        h(
          "div",
          { className: "connection-test" },
          h("span", null, "连接测试会发送一条简短请求，并记录实际用量。"),
          h(
            Button,
            {
              type: "button",
              disabled: busy || !saved,
              icon: "refresh",
              onClick: () => onTest().catch(() => {}),
            },
            "测试已保存的连接",
          ),
        ),
      ),
      h(
        "section",
        { className: "settings-card", id: "settings-performance" },
        h(
          "div",
          { className: "card-title" },
          h(Icon, { name: "settings" }),
          h("h2", null, "模型能力、速度与预算"),
        ),
        h(
          "div",
          { className: "settings-grid" },
          h(
            Field,
            { label: "上下文长度（token）" },
            h("input", {
              type: "number",
              min: 512,
              max: 4000000,
              value: form.llm.context_length,
              onChange: (e) => llm("context_length", +e.target.value),
            }),
          ),
          h(
            Field,
            {
              label: "单次输出上限（token）",
              hint: "思考过程可能占用此额度。仍受任务预算和上下文剩余空间限制。",
            },
            h("input", {
              type: "number",
              min: 64,
              max: 4000000,
              value: form.llm.max_output_tokens ?? 16384,
              onChange: (e) => llm("max_output_tokens", +e.target.value),
            }),
          ),
          h(
            Field,
            {
              label: "请求超时（秒）",
              hint: "模型请求最长等待时间，包含读取完整回答；等待时可随时停止。",
            },
            h("input", {
              type: "number",
              min: 1,
              max: 3600,
              value: form.llm.request_timeout_seconds ?? 600,
              onChange: (e) => llm("request_timeout_seconds", +e.target.value),
            }),
          ),
          h(Field, {label: "并行请求数", hint: "API 同时请求上限；本地另有 3 个预览工作位。复用原批次，不增加请求或自动重试；限流时调低。"},
            h("input", {type:"number",min:1,max:8,value:form.llm.parallel_requests ?? 3,onChange:e=>llm("parallel_requests",+e.target.value)})),
          h(Field, {label: "每批工具轮次上限", hint: "达到上限时暂停，保留已完成批次。"},
            h("input", {type:"number",min:1,max:32,value:form.max_iterations ?? 12,onChange:e=>set("max_iterations",+e.target.value)})),
          h(
            Field,
            {
              label: "每个任务的 token 预算",
              hint: "累计输入 + 输出；并行请求预先占用额度。未确认用量保留预留，留空表示不限。",
            },
            h("input", {
              type: "number",
              min: 1,
              value: form.token_budget ?? "",
              placeholder: "不限",
              onChange: (e) =>
                set("token_budget", e.target.value ? +e.target.value : null),
            }),
          ),
        ),
        h(
          "div",
          { className: "capability-toggles" },
          h(
            "label",
            null,
            h("input", {
              type: "checkbox",
              checked: form.llm.thinking_mode,
              onChange: (e) => llm("thinking_mode", e.target.checked),
            }),
            h(
              "span",
              null,
              h("b", null, "思考模式"),
              h("small", null, "按模型使用 thinking、reasoning 或原生思考参数；不可关闭的模型使用较低推理强度"),
            ),
          ),
          h(
            "label",
            null,
            h("input", {
              type: "checkbox",
              checked: form.llm.multimodal,
              onChange: (e) => llm("multimodal", e.target.checked),
            }),
            h(
              "span",
              null,
              h("b", null, "视觉能力"),
              h("small", null, "逐格式授权后才读取：图片缩略图、视频开头与中段最多 3 帧、PDF 首页／第二页／中间页（最多 3 页）。PDF 页面使用较高图像细节，计入模型视觉用量；Office 文本切片无需视觉模型。"),
              media && h("small",{className:"media-status"},media.native_pdf ? "PDF 系统原生预览已就绪 · 单文件上限 64 MiB · 最长解码 8 秒 · 加密文档不预览" : "本系统暂无原生 PDF 预览，PDF 按已授权的文件名与元数据分类"),
              h("small",{className:"media-status"},!media ? "正在检查本地预览能力…" : media.native_video ? `优先使用系统原生视频解码 · 最多三帧 · 512px。格式支持取决于系统编解码器。${media.ffmpeg ? "FFmpeg 后备已就绪。" : "无需安装 FFmpeg；解码不可用时使用已授权的文件信息。"}` : media.ffmpeg ? (media.ffprobe ? "FFmpeg 视频预览已就绪 · 开头 + 中段 · 512px" : "已找到 FFmpeg；缺少 FFprobe，仅采样开头") : "本系统暂无可用视频解码器；视频使用已授权的文件信息，其他预览可用。"),
            ),
          ),
        ),
      ),
      h("section",{className:"settings-card",id:"settings-pricing"},h("h2",null,"费用统计"),
        h("p",{className:"section-hint"},"默认匹配官方价格；中转、折扣或协议价格可手动覆盖。不同币种分别汇总。"),
        h("div",{className:"settings-grid"},
          h(Field, {label:"计费方式"}, h("select", {value:form.llm.pricing.mode || "auto",onChange:e=>pricing("mode",e.target.value)}, h("option",{value:"auto"},"自动使用官方价格"),h("option",{value:"manual"},"手动输入单价"))),
          form.llm.pricing.mode !== "manual" ? h(Field, {label:"官方结算币种",hint:"多币种服务请与账号结算地区一致；不会转换汇率。"}, h("select", {value:form.llm.pricing.official_currency || "",onChange:e=>pricing("official_currency",e.target.value || null)},h("option",{value:""},"自动选择"),h("option",{value:"CNY"},"人民币 CNY"),h("option",{value:"USD"},"美元 USD"))) : h(Field, {label:"手动价格币种"},h("select",{value:form.llm.pricing.currency || "USD",onChange:e=>pricing("currency",e.target.value)},h("option",{value:"CNY"},"人民币 CNY"),h("option",{value:"USD"},"美元 USD"))),
          form.llm.pricing.mode === "manual" && [
            ["input_per_1k_usd","普通输入"], ["output_per_1k_usd","输出（含思考）"],
            ["cached_input_per_1k","缓存读取"], ["cache_write_per_1k","缓存写入 / 5 分钟"], ["cache_write_1h_per_1k","缓存写入 / 1 小时"]
          ].map(([key,label],index)=>h(Field,{key,label:`${label}（${form.llm.pricing.currency || "USD"} / 百万 token）`,hint:index>1?"留空沿用普通输入价；0 表示免费。":null},h("input",{type:"number",min:0,step:"any",value:form.llm.pricing[key] == null ? "" : form.llm.pricing[key]*1000,placeholder:index>1?"沿用普通输入价":"0",onChange:e=>price(key,e.target.value==="" && index>1?null:+e.target.value)}))),
          h("div",{className:"field",style:{gridColumn:"1 / -1"}},h("strong",null,pricePreview ? (pricePreview.status==="unknown"?"价格未知":`${pricePreview.currency} · ${pricePreview.status==="manual"?"手动单价":"官方价格"}`) : "正在匹配价格…"),
            pricePreview?.rates && h("small",null,`当前普通输入 ${pricePreview.rates.input} / 输出 ${pricePreview.rates.output} / 缓存读取 ${pricePreview.rates.cached}，单位：每百万 token。`),
            pricePreview?.bands?.length>1 && h("small",null,pricePreview.bands.map(b=>`输入 ≥${b.min_input.toLocaleString()}：输入 ${b.input}，输出 ${b.output}，缓存 ${b.cached}`).join("；")),
            h("small",null,pricePreview?.note || ""),pricePreview?.verified_at && h("small",null,`价格表核对日期 ${pricePreview.verified_at}；按实际 usage 计算，优惠、税费与套餐抵扣以服务商账单为准。`),pricePreview?.source?.startsWith("https://") && h("a",{href:pricePreview.source,target:"_blank",rel:"noreferrer"},"查看官方价格依据")),
        )),
      h(
        "details",
        { className: "settings-card optional-settings", id: "settings-search" },
        h("summary", null, "可选 · 文件名联网检索"),
        h(
          "p",
          null,
          "可选 Tavily 兼容搜索服务。还需在每次命名任务中勾选启用；搜索服务费用独立于模型 token 成本。",
        ),
        h(
          "label",
          { className: "check-label" },
          h("input", {
            type: "checkbox",
            checked: form.search.enabled,
            onChange: (e) => search("enabled", e.target.checked),
          }),
          "允许任务选择联网检索",
        ),
        h(
          Field,
          { label: "搜索 API Endpoint" },
          h("input", {
            type: "url",
            value: form.search.endpoint,
            onChange: (e) => search("endpoint", e.target.value),
          }),
        ),
        h(
          Field,
          {
            label: "搜索 API Key",
            hint: "环境变量 DS_SEARCH_API_KEY 优先；留空保留已保存密钥。",
          },
          h("input", {
            type: "password",
            autoComplete: "off",
            value: form.search.api_key,
            placeholder: config.has_search_key
              ? "已配置搜索密钥"
              : "填写搜索服务密钥",
            onChange: (e) => search("api_key", e.target.value),
          }),
        ),
        h(
          "label",
          { className: "check-label" },
          h("input", {
            type: "checkbox",
            checked: clearSearchKey,
            onChange: (e) => {
              setClearSearchKey(e.target.checked);
              setSaved(false);
            },
          }),
          "清除配置文件中的搜索密钥",
        ),
      ),
      h(
        "section",
        { className: "settings-card", id: "settings-defaults" },
        h("h2", null, "默认扫描目录"),
        h(
          Field,
          { label: "目录路径" },
          h("input", {
            value: displayPath(form.scan_root),
            onChange: (e) => set("scan_root", e.target.value),
          }),
        ),
        config.scan_root_unavailable &&
          h(
            "p",
            { className: "warning" },
            "原配置的目录不在这台电脑上。首页已使用本机默认下载目录，请按需更新。",
          ),
      ),
      h(
        "div",
        { className: "settings-footer" },
        h("span", null, saved ? "设置已保存" : "有尚未保存的设置"),
        h(
          Button,
          {
            kind: "primary",
            type: "submit",
            icon: "check",
            disabled: busy || saved,
          },
          "保存设置",
        ),
      ),
    ),
  );
}

const CLEANUP_CATEGORIES = {large:"大文件", temporary:"临时 / 备份", incomplete:"未完成下载", installer:"旧安装 / 可执行文件", archive:"旧压缩包 / 镜像", copy:"疑似副本", empty:"空文件"};
function CleanupPanel({task,busy,act,recycleSupported}) {
  const [options,setOptions]=useState(task.cleanup_options || {large_mib:1024,stale_days:180,limit:200,categories:Object.keys(CLEANUP_CATEGORIES)});
  const [error,setError]=useState("");
  const [selected,setSelected]=useState(new Set()), [confirm,setConfirm]=useState(false);
  useEffect(()=>{setSelected(new Set());setConfirm(false);},[task.id,task.revision]);
  const records=task.recycled || [];
  const active=id=>records.some(r=>r.original_id===id && r.status!=="restored");
  const batches=[...new Set(records.map(r=>r.batch))].reverse();
  useEffect(()=>{if(task.cleanup_options)setOptions(task.cleanup_options);},[task.cleanup_options]);
  const dirty=JSON.stringify(options)!==JSON.stringify(task.cleanup_options);
  async function apply(ai) {
    setError("");
    try {await act(ai ? "cleanup_ai" : "cleanup_options",{options});}
    catch(e){setError(e.message);}
  }
  return h("section",{className:"cleanup-panel"},
    h("h3",null,"文件删除建议"),
    h("p",null,"这些是待人工核对的建议，不会自动删除。勾选并确认后移入系统回收站，可一键撤销。未修改不代表未使用，疑似副本未比较内容；整体保护目录内部文件不会列入。"),
    task.status==="completed" && h("div",{className:"cleanup-options"},
      h("div",{className:"cleanup-fields"},
        ...[["large_mib","大文件门槛（MiB）",1,1048576],["stale_days","至少未修改（天）",7,3650],["limit","最多显示候选",1,500]].map(([key,label,min,max])=>h(Field,{key,label},h("input",{type:"number",min,max,disabled:busy,value:options[key],onChange:e=>setOptions({...options,[key]:Number(e.target.value)})})))),
      h("div",{className:"cleanup-categories"},...Object.entries(CLEANUP_CATEGORIES).map(([key,label])=>h("label",{key,className:"check-label"},h("input",{type:"checkbox",disabled:busy,checked:options.categories.includes(key),onChange:e=>setOptions({...options,categories:e.target.checked?[...options.categories,key]:options.categories.filter(k=>k!==key)})}),label))),
      h("div",{className:"cleanup-buttons"},h(Button,{disabled:busy,onClick:()=>apply(false)},"更新本地筛选"),h(Button,{disabled:busy,icon:"spark",onClick:()=>apply(true)},"让 AI 复核候选")),
      h("small",null,"天数门槛用于旧安装包、压缩包、未完成下载、空文件等；大文件、疑似副本和小型 tmp/log 不受此门槛限制。AI 仅接收权限允许的名称 / 元数据，分批复核并计入预算，不读取正文。")),
    error && h("p",{className:"warning",role:"alert"},error),
    !recycleSupported && h("p",{className:"notice"},"当前平台尚未提供可撤销的系统回收站接口，已禁用删除。"),
    h("div",{className:"cleanup-buttons"},
      h("label",{className:"check-label"},h("input",{type:"checkbox","aria-label":"全选可回收建议",disabled:busy || !recycleSupported || task.status!=="completed",checked:task.cleanup.some(e=>!active(e.original_id)) && task.cleanup.filter(e=>!active(e.original_id)).every(e=>selected.has(e.original_id)),onChange:e=>setSelected(e.target.checked?new Set(task.cleanup.filter(e=>!active(e.original_id)).map(e=>e.original_id)):new Set())}),"全选可回收项"),
      h(Button,{disabled:busy || dirty || !recycleSupported || !selected.size || task.status!=="completed",onClick:()=>setConfirm(true)},`移入回收站（${selected.size}）`)),
    ...batches.map(batch=>{const rows=records.filter(r=>r.batch===batch),pending=rows.filter(r=>r.status!=="restored");return h("div",{key:batch,className:"notice recycle-batch"},
      h("strong",null,`回收批次 ${batch.slice(0,8)} · ${rows.length-pending.length}/${rows.length} 项已撤销`),
      pending.length>0 && h(Button,{disabled:busy,onClick:()=>act("cleanup_restore",{batch})},"一键撤销此批回收"),
      h("details",null,h("summary",null,"查看回收记录"),...rows.map(r=>h("p",{key:r.id},`${r.path} · ${{prepared:"尚未移动",staged:"已暂存，可撤销",recycling:"回收状态待核对，可撤销",trashed:"已移入回收站",restoring:"恢复待完成",restored:"已恢复原位"}[r.status] || r.status}`,r.error && h("small",{className:"warning"},r.error)))));}),
    records.length>0 && h("p",{className:"muted"},"撤销记录保存在本地，重启后仍可使用。回收站内按 DownloadSweeper-Recycle 标识分组，内部保留原文件名；不要清空回收站或删除任务记录。同名冲突会停止恢复。"),
    h("p",{className:"muted"},`显示 ${task.cleanup.length} / ${task.cleanup_summary?.matched ?? task.cleanup.length} 条候选 · 跳过 ${task.cleanup_summary?.protected_files || 0} 个整体保护目录内文件`),
    !task.cleanup.length && h("p",null,"当前筛选没有候选；可以调整门槛和类别。"),
    ...task.cleanup.map((e,i)=>h("article",{key:e.original_id || i,className:"cleanup-item"},
      h("label",{className:"check-label"},h("input",{type:"checkbox","aria-label":`回收 ${e.path}`,disabled:busy || !recycleSupported || task.status!=="completed" || active(e.original_id),checked:selected.has(e.original_id),onChange:event=>setSelected(previous=>{const next=new Set(previous);event.target.checked?next.add(e.original_id):next.delete(e.original_id);return next;})}),active(e.original_id)?"已有回收记录":"选择此文件"),
      h("div",null,...(e.categories || [e.category]).map(c=>h(Badge,{key:c,kind:"amber"},CLEANUP_CATEGORIES[c] || c))),
      h("strong",null,e.path),h("span",null,sizeLabel(e.size)+(e.age_days!=null?` · ${e.age_days} 天未修改`:"")),
      h("small",null,(e.source==="ai"?"AI · ":"本地规则 · ")+e.reason),
      e.ai_status && h("small",null,e.ai_status))),
    confirm && h(Modal,{title:`确认将 ${selected.size} 个文件移入系统回收站？`,onClose:()=>setConfirm(false)},
      h("p",null,"仅处理下面勾选的文件，移动前会重新全量校验。不会永久删除；若系统无法回收会停止。回收站被清空后无法撤销。"),
      h("ul",null,...task.cleanup.filter(e=>selected.has(e.original_id)).map(e=>h("li",{key:e.original_id},e.path))),
      h("div",{className:"modal-actions"},h(Button,{onClick:()=>setConfirm(false)},"取消"),h(Button,{kind:"primary",disabled:busy,onClick:async()=>{setConfirm(false);await act("cleanup_trash",{selected:[...selected],confirmed:true});}},"确认移入回收站"))));
}
function ArchiveViewer({archive,busy,onClose,onResume}) {
  const a=archive.data, contents=React.useMemo(()=>JSON.parse(a.payload),[a.payload]),t=contents.task;
  const [section,setSection]=useState("messages"),[shown,setShown]=useState(50),[root,setRoot]=useState(displayPath(t.root)),[error,setError]=useState("");
  const rows=section==="messages"?t.messages:section==="operations"?t.operations:contents.trajectory;
  return h(Modal,{title:"完整归档 · 只读",wide:true,onClose},
    h("p",null,displayPath(t.root)),
    h("p",null,`${t.entries.length} 项扫描记录 · ${t.messages.length} 条消息 · ${t.operations.length} 项操作 · ${contents.trajectory.length} 条轨迹`),
    h("p",{className:"muted"},"原始状态、调用费用和未确认用量均保留。完整性校验用于发现损坏，不代表对归档来源的认证。原目录不存在也可以查看。"),
    h("div",{className:"archive-tabs"},...[["messages","对话"],["operations","文件操作"],["trajectory","工作轨迹"]].map(([id,label])=>h(Button,{key:id,kind:section===id?"primary":"",onClick:()=>{setSection(id);setShown(50);}},label))),
    h("div",{className:"archive-records"},...rows.slice(0,shown).map((row,i)=>h("article",{key:i},
      section==="messages"?h(React.Fragment,null,h("small",null,`${row.scene} · ${row.role==="user"?"你":"AI"}`),h("p",null,row.content)):
      section==="operations"?h(React.Fragment,null,h("b",null,`${row.source} → ${row.destination}`),h("p",null,`${row.status} · ${row.reason}`)):
      h(React.Fragment,null,h("b",null,row.kind),h("small",null,row.timestamp),h("pre",null,JSON.stringify(row.detail,null,2))))),
      !rows.length && h("p",null,"没有对应记录。")),
    shown<rows.length && h(Button,{onClick:()=>setShown(shown+100)},`继续加载（已显示 ${shown}/${rows.length}）`),
    h("details",null,h("summary",null,"完整任务状态与 API 调用记录 JSON"),h("pre",{className:"archive-json"},JSON.stringify(t,null,2))),
    h(Button,{icon:"download",onClick:()=>download("downloadsweeper-archive-"+archive.id+".json",a)},"保存完整归档"),
    h("hr"),h("h3",null,"从归档创建新任务"),
    h("p",null,"复用规则和对话；清空可执行计划，重新扫描、规划和审查。原归档保持不变。"),
    h(Field,{label:"本机整理目录"},h("input",{value:root,onChange:e=>setRoot(e.target.value),disabled:busy})),
    error && h("p",{className:"warning",role:"alert"},error),
    h(Button,{disabled:busy,onClick:async()=>{try{await onResume(archive.id,root);}catch(e){setError(e.message);}}},"创建待扫描任务"));
}

function History({ tasks, busy, onOpen, onImport, onExport, onResume, read }) {
  const upload = useRef(null),
    [detail, setDetail] = useState(null),
    [archives, setArchives] = useState([]),
    [archive, setArchive] = useState(null),
    [archiveError, setArchiveError] = useState("");
  const reloadArchives = () => read("/api/archives").then(setArchives).catch(e => setArchiveError(e.message));
  useEffect(() => {if(!busy)reloadArchives();}, [busy]);
  return h(
    "div",
    { className: "history-wrap" },
    h(
      "div",
      { className: "page-title" },
      h(
        "div",
        null,
        h("div", { className: "eyebrow" }, "EVERY CHANGE HAS A HISTORY"),
        h("h1", null, "所有整理，都有迹可循"),
        h("p", null, "继续规则草稿，查看调用成本，或恢复已经执行的操作。"),
      ),
      h(
        Button,
        {
          icon: "download",
          disabled: busy,
          onClick: () => upload.current.click(),
        },
        "导入归档 / 旧会话",
      ),
      h("input", {
        type: "file",
        accept: ".json",
        ref: upload,
        hidden: true,
        onChange: async (e) => {
          const f = e.target.files[0];
          if (f) {
            try {
              if (f.size > 30 * 1024 * 1024) throw Error("归档超过 30 MiB 上限");
              const result = await onImport(JSON.parse(await f.text()));
              if (result?.archive_id) {
                await reloadArchives();
                setArchive({id:result.archive_id, data:await read("/api/archives/"+result.archive_id)});
              }
            } catch (error) {
              setArchiveError(error.message || "无法解析 JSON 会话文件");
            }
          }
          e.target.value = "";
        },
      }),
    ),
    archiveError && h("p", {className:"warning",role:"alert"}, archiveError),
    h("p", {className:"muted"}, "归档包含完整任务、对话、调用与 JSONL 轨迹；可能含私人路径和内容，请作为私有备份保存。导入归档不会执行或恢复文件。"),
    archives.length > 0 && h("section", {className:"archive-list"},
      h("h2", null, "只读归档"),
      ...archives.map(a => h("article", {key:a.id},
        h("div", null, h("b",null,displayPath(a.root)), h("small",null,`${a.messages} 条消息 · ${a.events} 条轨迹 · ${new Date(a.exported_at).toLocaleString()}`)),
        h(Button, {onClick:async()=>{try {setArchive({id:a.id,data:await read("/api/archives/"+a.id)});} catch(e){setArchiveError(e.message);}}}, "查看归档")))),
    archive && h(ArchiveViewer, {archive, busy, onClose:()=>setArchive(null), onResume}),
    !tasks.length
      ? h(
          "div",
          { className: "empty-state" },
          h(Icon, { name: "history", size: 40 }),
          h("h3", null, "你的第一份整理记录，从这里开始"),
          h("p", null, "创建任务后，扫描、规则与 AI 对话都会自动保存。"),
        )
      : h(
          "div",
          { className: "history-list" },
          ...tasks.map((t) =>
            h(
              "article",
              { key: t.id },
              h(
                "span",
                { className: "history-icon" },
                h(Icon, {
                  name: t.mode === "rename" ? "spark" : "folder",
                  size: 24,
                }),
              ),
              h(
                "div",
                { className: "history-info" },
                h(
                  "h3",
                  null,
                  displayPath(t.root).split(/[\\/]/).pop(),
                  h(
                    Badge,
                    { kind: t.status === "completed" ? "mint" : "" },
                    STATUS[t.status],
                  ),
                ),
                h("p", null, displayPath(t.root)),
                h(
                  "small",
                  null,
                  new Date(t.updated_at).toLocaleString() +
                    " · " +
                    t.file_count +
                    " 个文件 · " +
                    t.operations +
                    " 项操作",
                ),
              ),
              h(
                "div",
                { className: "history-cost" },
                h("b", null, costText(t.costs)),
                h(
                  "small",
                  null,
                  t.usage.prompt_tokens +
                    " 输入 / " +
                    t.usage.completion_tokens +
                    " 输出",
                ),
              ),
              h(
                "div",
                { className: "history-actions" },
                h(
                  Button,
                  { disabled: busy, onClick: () => onOpen(t.id) },
                  "打开任务",
                ),
                h(
                  "button",
                  { className: "text-btn", disabled:busy, onClick: () => onExport(t.id).catch(e=>setArchiveError(e.message)) },
                  "完整归档",
                ),
                h(
                  "button",
                  {
                    className: "text-btn",
                    onClick: async () =>
                      setDetail(await read("/api/tasks/" + t.id)),
                  },
                  "API 明细",
                ),
              ),
            ),
          ),
        ),
    detail &&
      h(
        Modal,
        { title: "API 调用记录", wide: true, onClose: () => setDetail(null) },
        h(
          "div",
          { className: "table-scroll" },
          h(
            "table",
            null,
            h(
              "thead",
              null,
              h(
                "tr",
                null,
                ...["用途 / 模型", "输入 token", "输出 token", "费用 / 依据"].map(
                  (x) => h("th", { key: x }, x),
                ),
              ),
            ),
            h(
              "tbody",
              null,
              ...detail.calls.map((c) =>
                h(
                  "tr",
                  { key: c.id },
                  h("td", null, c.purpose, h("small", null, c.model)),
                  h("td", null, c.usage.prompt_tokens, h("small",null,`缓存读 ${c.usage.cached_input_tokens || 0} / 写 ${c.usage.cache_write_tokens || 0}${c.usage.cache_details_known ? "" : " · 明细未知"}`)),
                  h("td", null, c.usage.completion_tokens),
                  h("td", null, callCost(c)),
                ),
              ),
              !detail.calls.length &&
                h(
                  "tr",
                  null,
                  h("td", { colSpan: 4 }, "这次任务没有 API 调用。"),
                ),
            ),
          ),
        ),
      ),
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(h(App));
