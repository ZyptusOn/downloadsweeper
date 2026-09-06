const {createElement: h, useState, useEffect} = React;

export function ruleCategory(rule, presets) {
  const saved = presets.find(p => p.id === rule.category);
  if (saved) return saved;
  const matched = presets.filter(p => rule.extensions.some(e => p.extensions.includes(e)));
  if (matched.length === 1) return matched[0];
  return {id: "", name: rule.extensions.length ? matched.length ? matched.map(p => p.name).join(" / ") : "自定义类别" : "全部格式", color: "#738378"};
}
export function extensionList(text) {
  return [...new Set(text.split(/[,，\s]+/).map(s => s.replace(/^\./, "").toLowerCase()).filter(Boolean))];
}
export function presetRule(preset, tier) {
  return {category: preset.id, extensions: [...preset.extensions], tier, min_bytes: null, max_bytes: null};
}

export function Permissions({task, busy, onSave, onDirty, presets = [], tiers}) {
  const [p, setP] = useState(task.permissions), [dirty, setDirty] = useState(false), [rawExtensions, setRawExtensions] = useState({});
  useEffect(() => {setP(task.permissions); setDirty(false); setRawExtensions({});}, [task.id, JSON.stringify(task.permissions)]);
  useEffect(() => {onDirty(dirty); return () => onDirty(false);}, [dirty]);
  const edit = patch => {setP({...p, ...patch}); setDirty(true);};
  const change = (index, patch) => edit({rules: p.rules.map((r, i) => i === index ? {...r, ...patch} : r)});
  const selectTier = (value, update, label) => h("select", {value, "aria-label": label, onChange: e => update(e.target.value)},
    Object.entries(tiers).map(([value, name]) => h("option", {key:value, value}, name)));
  const move = (index, direction) => {
    const rules = [...p.rules];
    [rules[index], rules[index + direction]] = [rules[index + direction], rules[index]];
    setRawExtensions({});
    edit({rules});
  };
  return h("section", {className:"permissions-card"},
    h("h3", null, "文件访问规则"),
    h("p", null, "按编号顺序匹配第一条规则。类别名称仅用于辨认，实际按下方扩展名和大小范围匹配。修改后保存生效。"),
    h("fieldset", {disabled:busy, className:"permission-fields"},
      h("div", {className:"permission-defaults"},
        h("label", null, "未匹配格式的权限", selectTier(p.default, value => edit({default:value}), "未匹配格式的权限")),
        h("label", null, "内容切片上限（字节）", h("input", {type:"number", min:0, max:65536, value:p.content_slice_bytes,
          onChange:e=>edit({content_slice_bytes:+e.target.value})}))),
      h("h4", null, "添加类别预设"),
      h("p", {className:"permission-explanation"}, "预设填入常见扩展名，可继续增删。新增规则继承未匹配格式的权限、追加到末尾，既有规则优先；可用上移／下移调整顺序。"),
      h("div", {className:"permission-presets"}, presets.map(preset => h("button", {
        key:preset.id, type:"button", className:"permission-preset", style:{"--category-color":preset.color},
        "aria-label":`添加${preset.name}规则`, onClick:()=>edit({rules:[...p.rules,presetRule(preset,p.default)]})},
        h("strong", null, preset.name), h("span", {title:preset.extensions.join(", ")}, preset.extensions.join(" · "))))),
      h("div", {className:"permission-rule-heading"}, h("h4", null, `已配置 ${p.rules.length} 条规则`),
        h("button", {className:"btn", type:"button", onClick:()=>edit({rules:[...p.rules,{extensions:[],tier:p.default,min_bytes:null,max_bytes:null}]})}, "+ 自定义规则")),
      h("div", {className:"permission-rules"}, p.rules.map((rule, index) => {
        const category = ruleCategory(rule, presets);
        const shadowed = p.rules.slice(0,index).some(r => !r.extensions.length && r.min_bytes == null && r.max_bytes == null);
        return h("article", {className:"permission-rule", key:index, style:{"--category-color":category.color}},
          h("header", {className:"permission-rule-title"}, h("strong", null, `${index+1}. ${category.name}`),
            h("div", {className:"permission-rule-tools"},
              h("button", {type:"button", disabled:index===0, "aria-label":`上移规则 ${index+1}`, onClick:()=>move(index,-1)}, "↑"),
              h("button", {type:"button", disabled:index===p.rules.length-1, "aria-label":`下移规则 ${index+1}`, onClick:()=>move(index,1)}, "↓"),
              h("button", {type:"button", "aria-label":`移除规则 ${index+1}`, onClick:()=>{setRawExtensions({});edit({rules:p.rules.filter((_,i)=>i!==index)});}}, "×"))),
          h("label", null, "类别名称", h("select", {value:rule.category || "", "aria-label":`规则 ${index+1} 类别`,
            onChange:e=>change(index,{category:e.target.value || null})}, h("option", {value:""}, "根据扩展名辨认"),
            presets.map(v=>h("option", {key:v.id,value:v.id},v.name)))),
          h("label", null, "具体扩展名（可编辑）", h("input", {"aria-label":`规则 ${index+1} 扩展名`,
            value:rawExtensions[index] ?? rule.extensions.join(", "), placeholder:"例如 mp4, mkv, mov；留空匹配全部格式",
            onChange:e=>{
              setRawExtensions({...rawExtensions,[index]:e.target.value});
              change(index,{extensions:extensionList(e.target.value),category:rule.category || category.id || null});
            }})),
          h("label", null, "允许 Agent 读取", selectTier(rule.tier, tier=>change(index,{tier}),`规则 ${index+1} 读取权限`)),
          h("div", {className:"size-range"},
            h("label", null, "最小字节", h("input", {type:"number",min:0,placeholder:"不限",value:rule.min_bytes ?? "", "aria-label":`规则 ${index+1} 最小字节`,onChange:e=>change(index,{min_bytes:e.target.value===""?null:+e.target.value})})),
            h("label", null, "最大字节", h("input", {type:"number",min:0,placeholder:"不限",value:rule.max_bytes ?? "", "aria-label":`规则 ${index+1} 最大字节`,onChange:e=>change(index,{max_bytes:e.target.value===""?null:+e.target.value})}))),
          !rule.extensions.length && h("small", {className:"warning"}, "这条规则匹配所有格式，请留意它与后续规则的顺序。"),
          shadowed && h("small", {className:"warning"}, "前面有一条不限大小的全部格式规则，本条不会命中；如需生效请上移。"));
      })),
      h("p", {className:"permission-explanation"}, "文件夹预设（@folder）控制目录名称及内部摘要的读取上限；内部每个文件还需符合自身格式权限。文本与 Office 正文需要内容切片权限；图片／视频／PDF 页面预览还需模型支持视觉。PDF 只采样少量页面，不提取全文或运行 OCR。预设覆盖某种扩展名，不表示该格式一定支持内容解析。")),
    h("div", {className:"permission-actions"}, h("button", {type:"button", className:"btn primary", disabled:busy || !dirty,
      onClick:async()=>{if(await onSave(p))setDirty(false);}}, dirty ? "保存权限更改" : "已保存"),
      dirty && h("small", null, "请保存后进入下一步")));
}
