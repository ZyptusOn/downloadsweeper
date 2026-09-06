//! The model proposes graph edits; Rust supplies preserved fields and checks the result.
use super::*;

pub(super) const INSTRUCTIONS: &str = r#"tree 场景编辑的是完整目标目录树，不仅是一级扩展名表。你可以新增任意层级节点、改名、修改 note（分类描述/备注）、调整父子关系或删除节点。语义分类的描述应说明收纳范围、排除范围和不确定文件如何处理。
changes 中每项格式为 {"kind":"node","target":"节点id","after":对象或null}：
1. 新增：target 使用当前 nodes 中不存在的唯一 id，after 必须包含 name、parent、rule_type；可包含 extensions、note。id 若提供必须与 target 相同。新增一级示例：{"kind":"node","target":"new-audio","after":{"name":"音频","parent":"root","rule_type":"simple","extensions":["mp3","wav","flac"],"note":"音乐与录音；内部子目录可进一步细分。"}}。新增子级例如 {"kind":"node","target":"new-reference","after":{"name":"参考资料","parent":"已有一级节点的实际id","rule_type":"complex","extensions":[],"note":"供研究与查阅的资料；工作交付物交给工作目录；依据不足时保留在父目录。"}}。
2. 修改：target 为已有 id，after 只需包含改动字段，例如 {"kind":"node","target":"已有节点的实际id","after":{"note":"新的分类描述"}}。修改扩展名时 extensions 是修改后的完整数组。已有 examples 和画布 position 由 Rust 保留，不要输出它们。
3. 删除：target 为已有 id，after=null。仅删除分类规则；文件不会删除，保留的子节点会断开。需要迁移子节点时同时输出其 parent 改动。
可在同一批新增父节点和子节点，子节点 parent 指向新父节点的 target。parent="root" 是一级目录，必须 simple，extensions 不得与其他一级重复；二级及更深可 complex。根节点 root 不能修改或删除。每个节点只输出一项，最多100项，不重复输出未变化节点。
新增前按 parent 与 name 检查现有节点：同一父目录下已有同名节点（例如模板中的“照片”）时，必须复用其实际 id；修改备注或添加子目录，不要换一个新 id 重建同名节点。不同父目录下可以有同名节点。
新增节点不要编造 examples 或实际路径，mapping 默认 null；仅已有 class=container 的一级实际目录可以用于 mapping。原子保护约束实际目录内部文件，不禁止在目标树上新增分类节点；保持保护目录完整，与为可拆散文件创建子分类可同时成立。此场景不能修改实际目录的保护类型。
name 是单个文件夹名称，不是路径；组合主题请用顿号，不得包含 /、反斜杠、冒号、问号、星号、双引号、尖括号或竖线，不能以句点结尾。层级只能通过 parent 表达。
只返回与用户需求和观察证据相符的改动。明确要求新建、细分或修改描述时，必须以 changes 表达这些编辑，不能只在 message 中描述愿望或仅补全扩展名。说明没有必要改动时给出具体理由。所有改动等待用户合并。"#;

pub(super) const DESIGN_INSTRUCTIONS: &str = "本次任务是根据分类型检查摘要逐步设计可用的多层分类结构。本轮最多改动12个节点，优先补齐扫描摘要中数量较多但缺失的一级格式分类，再改善最需要细分的1至2个分支，不要尝试一轮穷尽所有类别；用户指定范围时只处理该范围。其他可改进的类别在 message 中简短说明可留待下一轮，不要把尚未输出的改动说成已完成。对照所选分支对应的 file_inspection 与现有节点：对摘要有依据且现有子目录不能表达的用途/主题，新增或调整二级及更深语义节点，并写清 note；能沿用的节点补充具体分类边界；现有模板不适用时允许改名、重接或删除。缺失音频等类别时，应直接新增 parent=root、rule_type=simple 的一级目录，extensions 列出该类别实际扩展名；若扩展名已分配给其他一级节点，同批移除旧归属，避免重复。一级目录及它的复杂规则子目录可在同批建议中创建，不必依附已有的其他类别。一级仍按扩展名分流；完整原子文件夹可以根据整体内容归入适合的一级类别，不拆散。减少移动是选择可复用实际容器时的考虑，不是禁止设计子目录。不要把这次结构设计降格为仅补全一级扩展名。无需为了创建而创建，文件少或依据不足的类别可以保持粗分类，并在 message 中说明。只返回真实需要的节点改动，不要仅在回答中罗列分类建议。";

pub(super) fn normalize(task: &Task, target: &str, after: Value) -> Result<Value> {
    ensure!(
        !target.is_empty() && target != "root",
        "AI 不能修改根节点或使用空节点 ID"
    );
    let old = task.nodes.iter().find(|n| n.id == target);
    if after.is_null() {
        ensure!(old.is_some(), "AI 要删除的节点不存在：{target}");
        return Ok(Value::Null);
    }
    let patch = after
        .as_object()
        .context("节点 after 必须是字段对象或 null")?;
    if let Some(id) = patch.get("id") {
        ensure!(id.as_str() == Some(target), "AI 节点 ID 不匹配");
    }
    let mut value = old
        .map(|n| json!(n))
        .unwrap_or_else(|| json!({"id":target}));
    if old.is_none() {
        ensure!(
            patch.contains_key("name")
                && patch.contains_key("parent")
                && patch.contains_key("rule_type"),
            "新增节点必须指定 name、parent 和 rule_type"
        );
        ensure!(
            patch
                .get("examples")
                .is_none_or(|v| v.as_array().is_some_and(|a| a.is_empty())),
            "新增节点不能编造文件示例，请在界面选择实际文件"
        );
    }
    for (key, field) in patch {
        match key.as_str() {
            "name" | "parent" | "rule_type" | "extensions" | "note" | "mapping" => {
                value[key] = field.clone();
            }
            // Context-only fields must never replace locally held references/positions.
            "id" | "examples" | "position" | "example_count" | "example_context" => {}
            _ => anyhow::bail!("AI 节点字段不支持：{key}，分类描述请使用 note"),
        }
    }
    let mut node: Node = serde_json::from_value(value).context("AI 节点字段格式无效")?;
    // Models often join themes with '/' or ':'. Keep one reviewed folder name;
    // never interpret the generated label as a filesystem path.
    node.name = portable_label(&node.name);
    Ok(json!(node))
}

fn portable_label(name: &str) -> String {
    let mut name: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' => '／',
            '\\' => '＼',
            ':' => '：',
            '*' => '＊',
            '?' => '？',
            '"' => '＂',
            '<' => '＜',
            '>' => '＞',
            '|' => '｜',
            _ => c,
        })
        .collect();
    if name.ends_with('.') {
        name.pop();
        name.push('．');
    }
    name
}

/// Models sometimes recreate a template node under a fresh ID. Resolve only compatible
/// additions, never delete or merge two existing user nodes. The final graph is still validated.
pub(super) fn reconcile_additions(task: &Task, changes: &mut Vec<Change>) -> Result<Vec<Value>> {
    let existing: HashSet<_> = task.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut adjustments = vec![];
    loop {
        let mut projected = task.nodes.clone();
        for change in changes.iter() {
            if change.kind != "node" { continue; }
            if change.after.is_null() {
                projected.retain(|n| n.id != change.target);
            } else {
                let node: Node = serde_json::from_value(change.after.clone())?;
                if let Some(old) = projected.iter_mut().find(|n| n.id == node.id) { *old = node; }
                else { projected.push(node); }
            }
        }
        // Existing IDs precede newly proposed IDs, regardless of the model's output order.
        projected.sort_by_key(|n| !existing.contains(n.id.as_str()));
        let mut siblings: HashMap<(String, String), Vec<Node>> = HashMap::new();
        let mut duplicate = None;
        for node in projected {
            let Some(parent) = &node.parent else { continue; };
            let peers = siblings.entry((parent.clone(), node.name.to_lowercase())).or_default();
            if !existing.contains(node.id.as_str()) {
                let extensions = |n: &Node| n.extensions.iter().map(|e| e.to_lowercase()).collect::<std::collections::BTreeSet<_>>();
                if let Some(winner) = peers.iter().find(|p| p.rule_type == node.rule_type
                    && extensions(p) == extensions(&node)
                    && (node.mapping.is_none() || node.mapping == p.mapping)) {
                    duplicate = Some((winner.clone(), node));
                    break;
                }
            }
            peers.push(node);
        }
        let Some((mut winner, duplicate)) = duplicate else { break; };
        let explicit_note = changes.iter().any(|c| c.target == winner.id
            && !c.after["note"].as_str().unwrap_or("").is_empty()
            && c.before["note"] != c.after["note"]);
        if !explicit_note && !duplicate.note.is_empty() { winner.note = duplicate.note.clone(); }
        changes.retain(|c| c.kind != "node" || c.target != duplicate.id);
        for change in changes.iter_mut().filter(|c| c.kind == "placement") {
            if change.after["node_id"].as_str() == Some(&duplicate.id) { change.after["node_id"] = json!(winner.id); }
        }
        // This also fixes grandchildren when a duplicate parent and child are both recreated.
        for change in changes.iter_mut().filter(|c| c.kind == "node" && !c.after.is_null()) {
            if change.after["parent"].as_str() == Some(&duplicate.id) {
                change.after["parent"] = json!(winner.id);
            }
        }
        if let Some(change) = changes.iter_mut().find(|c| c.target == winner.id) {
            change.after = json!(winner);
        } else {
            let original = task.nodes.iter().find(|n| n.id == winner.id).context("复用节点不存在")?;
            changes.push(Change { id: uuid::Uuid::new_v4().to_string(), kind: "node".into(),
                target: winner.id.clone(), label: original.name.clone(), before: json!(original), after: json!(winner) });
        }
        adjustments.push(json!({"discarded_id":duplicate.id,"reused_id":winner.id,"name":winner.name}));
    }
    changes.retain(|c| c.kind == "placement" || c.before != c.after);
    Ok(adjustments)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(id: &str, parent: &str, name: &str) -> Node {
        Node { id:id.into(), parent:Some(parent.into()), name:name.into(),
            rule_type:crate::tree::RuleType::Complex, extensions:vec![], note:"原有备注".into(),
            examples:vec![], mapping:None, position:None }
    }
    fn task(nodes: Vec<Node>) -> Task {
        let mut task = Task::new(std::env::temp_dir(), "desktop", crate::permission::PermissionConfig::default()).unwrap();
        task.nodes = nodes;
        task
    }
    fn change(task: &Task, id: &str, after: Value) -> Change {
        Change {id:id.into(), kind:"node".into(), target:id.into(), label:id.into(),
            before:task.nodes.iter().find(|n| n.id==id).map(|n|json!(n)).unwrap_or(Value::Null),
            after:normalize(task,id,after).unwrap()}
    }
    #[test]
    fn duplicate_template_nodes_reuse_ids_and_rewire_descendants_without_losing_local_fields() {
        let mut photos = node("photos","images","照片");
        photos.examples = vec!["sample.jpg".into()]; photos.position = Some([24.0,48.0]);
        let task = task(vec![photos.clone(),node("family","photos","家庭")]);
        let original = task.nodes.clone();
        let mut changes = vec![
            change(&task,"travel",json!(node("travel","new-family","旅行"))),
            change(&task,"new-family",json!(node("new-family","new-photos","家庭"))),
            change(&task,"new-photos",json!({"name":" 照片 ","parent":"images","rule_type":"complex","note":"新的分类说明"})),
        ];
        assert_eq!(reconcile_additions(&task,&mut changes).unwrap().len(),2);
        assert_eq!(task.nodes,original);
        let edited = changes.iter().find(|c| c.target=="photos").unwrap();
        assert_eq!(edited.after["note"],"新的分类说明");
        assert_eq!(edited.after["examples"],json!(photos.examples));
        assert_eq!(edited.after["position"],json!(photos.position));
        assert_eq!(changes.iter().find(|c| c.target=="travel").unwrap().after["parent"],"family");
        assert!(changes.iter().all(|c| !c.target.starts_with("new-")));
        assert!(reconcile_additions(&task,&mut changes).unwrap().is_empty());
    }
    #[test]
    fn repeated_new_nodes_share_one_reviewable_creation_case_insensitively() {
        let task = task(vec![]);
        let mut changes=vec![change(&task,"a",json!(node("a","root","Album"))),
            change(&task,"b",json!(node("b","root","album"))),
            change(&task,"c",json!(node("c","b","旅行")))];
        assert_eq!(reconcile_additions(&task,&mut changes).unwrap().len(),1);
        assert_eq!(changes.len(),2);
        assert_eq!(changes.iter().find(|c|c.target=="c").unwrap().after["parent"],"a");
    }
    #[test]
    fn exact_duplicate_becomes_noop_but_incompatible_rules_and_existing_nodes_are_not_merged() {
        let task = task(vec![node("photos","images","照片"),node("other","images","其他")]);
        let mut changes=vec![change(&task,"copy",json!(node("copy","images","照片")))];
        assert_eq!(reconcile_additions(&task,&mut changes).unwrap().len(),1);
        assert!(changes.is_empty());
        let mut changes=vec![change(&task,"other",json!({"name":"照片"}))];
        assert!(reconcile_additions(&task,&mut changes).unwrap().is_empty());
        let mut changes=vec![change(&task,"copy",json!({"name":"照片","parent":"images","rule_type":"simple","extensions":["png"]}))];
        assert!(reconcile_additions(&task,&mut changes).unwrap().is_empty());
        let mut changes=vec![change(&task,"photos",json!({"name":"历史照片"})),
            change(&task,"copy",json!(node("copy","images","照片")))];
        assert!(reconcile_additions(&task,&mut changes).unwrap().is_empty(),"A planned rename vacates the old name");
    }
    #[test]
    fn generated_theme_labels_are_single_portable_folder_names() {
        for (input, expected) in [
            ("校园纪实/学业", "校园纪实／学业"),
            ("视频:素材?", "视频：素材？"),
            ("  参考资料. ", "参考资料．"),
            ("a\\b*<c>|d\"", "a＼b＊＜c＞｜d＂"),
        ] {
            let label = portable_label(input);
            assert_eq!(label, expected);
            valid_name(&label).unwrap();
        }
        // Platform constraints still reject device names and empty labels.
        assert!(valid_name(&portable_label("CON")).is_err());
        assert!(valid_name(&portable_label("   ")).is_err());
    }
}
