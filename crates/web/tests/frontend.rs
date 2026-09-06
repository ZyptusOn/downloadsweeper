//! Rust assertions against the shipped JavaScript, using a Rust ECMAScript engine.
//! No Node/Python subprocess and no reimplementation of layout/selection algorithms.
use boa_engine::{builtins::promise::PromiseState, property::Attribute, Context, Module, Source};
use serde_json::{json, Value};
struct Js(Context);
impl Js {
    fn new() -> Self {
        let mut js = Self(Context::default());
        js.run("globalThis.React={memo:c=>c}; globalThis.window={ReactFlow:{}};");
        js
    }
    fn run(&mut self, code: &str) {
        self.0
            .eval(Source::from_bytes(code))
            .unwrap_or_else(|e| panic!("JavaScript: {e}\n{code}"));
    }
    fn load(&mut self, name: &str, source: &str) {
        let m = Module::parse(Source::from_bytes(source), None, &mut self.0).unwrap();
        let promise = m.load_link_evaluate(&mut self.0);
        self.0.run_jobs().unwrap();
        assert!(
            matches!(promise.state(), PromiseState::Fulfilled(_)),
            "{name}: {:?}",
            promise.state()
        );
        let ns = m.namespace(&mut self.0);
        self.0
            .register_global_property(boa_engine::JsString::from(name), ns, Attribute::all())
            .unwrap();
    }
    fn value(&mut self, expression: &str) -> Value {
        let v = self
            .0
            .eval(Source::from_bytes(&format!("JSON.stringify({expression})")))
            .unwrap();
        serde_json::from_str(&v.to_string(&mut self.0).unwrap().to_std_string_escaped()).unwrap()
    }
    fn yes(&mut self, expression: &str) {
        assert_eq!(self.value(expression), json!(true), "{expression}");
    }
}
const GRAPH: &str = include_str!("../../../frontend/graph.js");
const APP: &str = include_str!("../../../frontend/app.js");
#[test]
fn every_shipped_frontend_module_parses() {
    for e in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../frontend")).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_some_and(|x| x == "js") {
            let source = std::fs::read_to_string(&path).unwrap();
            Module::parse(Source::from_bytes(&source), None, &mut Context::default())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
}
#[test]
fn graph_rows_measured_cards_depth_folding_and_detached_dragging() {
    let mut js = Js::new();
    js.load("graph", GRAPH);
    js.run("const {layout,defaultCollapsed,branchState,dragDetached,visibleEntries}=graph; const nodes=[{id:'a',parent:'root'},{id:'b',parent:'root'},{id:'a2',parent:'a'},{id:'a3',parent:'a2'},{id:'a4',parent:'a3'},{id:'a5',parent:'a4'}];");
    assert_eq!(
        js.value("[...defaultCollapsed(nodes)].sort()"),
        json!(["a3", "a4"])
    );
    js.run("const state=branchState([...nodes,{id:'new',parent:'a5'}],new Map([['a',true],['a3',false]]));");
    for expr in [
        "state.has('a')",
        "!state.has('a3')",
        "state.has('a5')",
        "state.has('a4')",
    ] {
        js.yes(expr);
    }
    js.run("const dimensions=new Map([['root',{width:360,height:90}],['a',{width:360,height:550}],['b',{width:360,height:120}],['a2',{width:360,height:90}]]); const visible=nodes.slice(0,3); const positions=layout(visible,'target',new Set(visible.map(n=>n.id)),new Map(),dimensions); const centre=id=>positions.get(id).x+(dimensions.get(id)?.width||360)/2;");
    for expr in ["centre('root')===0","centre('a')===centre('a2')","positions.get('a').y===positions.get('b').y","positions.get('a').x+dimensions.get('a').width+24<=positions.get('b').x","visible.every(n=>positions.get(n.id).y>=positions.get(n.parent).y+dimensions.get(n.parent).height+60)","(positions.get('a').x+positions.get('b').x+dimensions.get('b').width)/2===0"]{js.yes(expr);}
    js.run("const detached=layout([...visible,{id:'orphan',parent:null},{id:'child',parent:'orphan'}],'target');");
    js.yes("detached.has('orphan')&&detached.get('child').y>detached.get('orphan').y");
    assert_eq!(
        js.value("[...layout(visible.map(n=>({...n,position:[9999,9999]})),'target')]"),
        js.value("[...layout(visible,'target')]")
    );
    js.run("const wide=[...visible,{id:'a2b',parent:'a'},{id:'b2',parent:'b'},{id:'b3',parent:'b2'},{id:'b4',parent:'b3'}]; const measured=new Map(wide.map((n,i)=>[n.id,{width:320+i*11,height:120+i*67}])); measured.set('root',{width:360,height:100});");
    for kind in ["actual", "target"] {
        let cards=js.value(&format!("[...layout(wide,'{kind}',new Set(),new Map(),measured)].map(([id,p])=>({{id,...p,...measured.get(id)}}))"));
        let cards = cards.as_array().unwrap();
        for (i, a) in cards.iter().enumerate() {
            for b in &cards[i + 1..] {
                let f = |v: &Value, k: &str| v[k].as_f64().unwrap();
                assert!(
                    f(a, "x") + f(a, "width") <= f(b, "x")
                        || f(b, "x") + f(b, "width") <= f(a, "x")
                        || f(a, "y") + f(a, "height") <= f(b, "y")
                        || f(b, "y") + f(b, "height") <= f(a, "y"),
                    "{kind}: {a} overlaps {b}"
                );
            }
        }
    }
    js.run("const forest=[...visible,{id:'free',parent:null,position:[-120,500]},{id:'child',parent:'free'},{id:'leaf',parent:'child'}];const original=layout(forest,'target');const cards=[...original].map(([id,position])=>({id,position}));const dragged=dragDetached(cards,forest,[{type:'position',id:'free',position:{x:280,y:650}}]);const moved=new Map(dragged.map(n=>[n.id,n.position]));");
    assert_eq!(js.value("original.get('free')"), json!({"x":-120,"y":500}));
    for id in ["free", "child", "leaf"] {
        let old = js.value(&format!("original.get('{id}')"));
        let moved = js.value(&format!("moved.get('{id}')"));
        assert_eq!(
            moved["x"].as_f64().unwrap(),
            old["x"].as_f64().unwrap() + 400.
        );
        assert_eq!(
            moved["y"].as_f64().unwrap(),
            old["y"].as_f64().unwrap() + 150.
        );
    }
    assert_eq!(js.value("moved.get('a')"), js.value("original.get('a')"));
    assert_eq!(
        js.value("dragDetached(cards,forest,[{type:'position',id:'child',position:{x:0,y:0}}])"),
        js.value("cards")
    );
    js.run("const saved=forest.map(n=>n.id==='free'?{...n,position:[280,650]}:n);const reattached=saved.map(n=>n.id==='free'?{...n,parent:'a'}:n);");
    assert_eq!(
        js.value("[...layout(saved,'target')]"),
        js.value("[...moved]")
    );
    assert_eq!(
        js.value("[...layout(reattached,'target')]"),
        js.value("[...layout(reattached.map(n=>({...n,position:null})),'target')]")
    );
    assert_eq!(
        js.value(
            "dragDetached(cards,forest,[{type:'position',id:'free',position:{x:Infinity,y:0}}])"
        ),
        js.value("cards")
    );
    js.run("const entries=[{kind:'file',extension:'URL'},{kind:'file',extension:'lnk'},{kind:'file',extension:'txt'},{kind:'directory',extension:'url'}];");
    assert_eq!(
        js.value("visibleEntries({mode:'desktop',entries}).length"),
        2
    );
    assert_eq!(
        js.value("visibleEntries({mode:'organize',entries}).length"),
        4
    );
}
#[test]
fn permission_presets_keep_names_colors_and_explicit_tiers() {
    let mut js = Js::new();
    js.load(
        "permissions",
        include_str!("../../../frontend/permissions.js"),
    );
    js.run("const {ruleCategory,extensionList,presetRule}=permissions;const presets=[{id:'video',name:'视频',color:'#7560ad',extensions:['mp4','mkv']},{id:'text',name:'文本',color:'#278477',extensions:['txt','md']}];const rule=presetRule(presets[0],'none');");
    assert_eq!(
        js.value("extensionList('.MP4，mkv  mp4, .custommovie')"),
        json!(["mp4", "mkv", "custommovie"])
    );
    assert_eq!(js.value("rule.tier"), "none");
    js.run("rule.extensions=['custommovie'];");
    assert_eq!(js.value("ruleCategory(rule,presets).name"), "视频");
    assert_eq!(js.value("presets[0].extensions"), json!(["mp4", "mkv"]));
    assert_eq!(
        js.value("ruleCategory({extensions:['md']},presets).name"),
        "文本"
    );
    assert_eq!(
        js.value("ruleCategory({extensions:[]},presets).name"),
        "全部格式"
    );
    assert_eq!(
        js.value("ruleCategory(JSON.parse(JSON.stringify(rule)),presets).color"),
        "#7560ad"
    );
}
#[test]
fn proposal_parent_descendant_extension_and_placement_dependencies() {
    let mut js = Js::new();
    js.load(
        "selection",
        include_str!("../../../frontend/proposal_selection.js"),
    );
    js.run("const {proposalDependencies,validProposalSelection}=selection;const add=(id,parent)=>({id,target:id,kind:'node',before:null,after:{parent,extensions:[]}});const changes=[add('grandchild','child'),add('child','parent'),add('parent','root'),add('independent','root')];const deps=proposalDependencies(changes);let selected=validProposalSelection(new Set(['grandchild','child','independent']),deps);");
    assert_eq!(js.value("[...selected]"), json!(["independent"]));
    js.run("selected.add('grandchild');");
    js.yes("!validProposalSelection(selected,deps).has('grandchild')");
    js.run("selected.add('parent');selected=validProposalSelection(selected,deps);");
    js.yes("!selected.has('child')");
    js.run("selected.add('child');selected.add('grandchild');");
    assert_eq!(js.value("validProposalSelection(selected,deps).size"), 4);
    js.run("const old={id:'old',parent:'root',extensions:['mp3','zip']};const split=[{id:'release',target:'old',kind:'node',before:old,after:{...old,extensions:['zip']}},{...add('music','root'),after:{parent:'root',extensions:['mp3']}}];const placement={id:'place',kind:'placement',target:'file',after:{node_id:'child'}};");
    assert_eq!(
        js.value("[...proposalDependencies(split,[old]).get('music')]"),
        json!(["release"])
    );
    assert_eq!(
        js.value(
            "validProposalSelection(new Set(['music']),proposalDependencies(split,[old])).size"
        ),
        0
    );
    js.yes("!validProposalSelection(new Set(['place']),proposalDependencies([...changes,placement])).has('place')");
}
#[test]
fn parallel_counts_and_live_completed_batch_render_without_private_decisions() {
    let mut js = Js::new();
    js.load(
        "progress",
        include_str!("../../../frontend/parallel_progress.js"),
    );
    js.run("const {parallelSummary}=progress;const s=parallelSummary({batches:[{status:'complete',files:2},{status:'running',files:8},{status:'running',files:4},{status:'pending',files:6}]});");
    assert_eq!(
        js.value("[s.percent,s.active.length,s.queued.length,s.completed]"),
        json!([10, 2, 1, 2])
    );
    assert_eq!(js.value("parallelSummary({batches:[]}).percent"), 0);
    assert_eq!(
        js.value("parallelSummary({batches:[{status:'complete',files:10}]}).percent"),
        100
    );
    let start = APP.find("function ClassificationProgress(").unwrap();
    let end = APP.find("function InspectionProgress(").unwrap();
    js.run("const h=(type,props,...children)=>({type,props,children});const ParallelProgress=()=>null;");
    js.run(&APP[start..end]);
    js.run("ClassificationProgress({classification:{status:'running',total:10,batches:[]},job:{kind:'plan_ai',parallel:{total_files:10,completed_files:2,batches:[{id:'b1',branch:'video',files:2,status:'complete'},{id:'b2',branch:'video',files:8,status:'running'}]}}});");
}
#[test]
fn archive_opaque_payload_survives_browser_json_roundtrip() {
    let mut js = Js::new();
    let payload = r#"{"integer":18446744073709551615,"whole_float":1.0,"negative_zero":-0.0}"#;
    let archive = json!({"payload":payload});
    assert_eq!(
        js.value(&format!("JSON.parse(JSON.stringify({archive}))")),
        archive
    );
}
