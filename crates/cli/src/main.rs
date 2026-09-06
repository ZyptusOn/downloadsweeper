//! CLI adapter for the six-stage workflow used by the web and desktop UI.
use anyhow::{ensure, Context, Result};
use ds_engine::{
    config::AppConfig,
    safe_fs::{self, TaskStore},
    workflow::Task,
    workflow_ai,
};
use serde_json::json;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

fn main() -> Result<()> {
    ds_engine::evidence::dispatch_helper();
    run()
}
#[tokio::main]
async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");
    if command == "help" || command == "--help" {
        println!("DownloadSweeper · Rust 文件整理\n\n浏览器界面：cargo run -p ds-web -- --open\n\nCLI（与 GUI 共用 .ds-data，同一时刻只运行一个进程）：\n  ds create <绝对目录> [organize|desktop|rename] 创建任务并扫描\n  ds list                     历史任务\n  ds show <ID>                完整任务 JSON\n  ds scan <ID>                刷新扫描，保留规则\n  ds next <ID>                进入下一阶段；桌面与下载目录均使用完整六步流程\n  ds back <ID> <0|1|2|3|4>    返回阶段；仅返回上游时作废计划\n  ds export <ID> <文件>       导出任务与完整轨迹归档\n  ds import <文件>            归档只读保存；旧 Task JSON 创建待重扫任务\n  ds archives                 列出只读归档\n  ds archive-show <ID>         查看完整归档 JSON\n  ds resume <归档ID> [目录]    从归档创建待重扫任务\n  ds cleanup <ID> [--ai]       本地清理建议或 AI 复核；不删除\n  ds update <ID> <文件>       从 ds show 的 JSON 更新当前阶段规则\n  ds plan <ID> [--ai]         重新生成规则计划或批量分类；--batch-size 64 调批量，--thinking 开启思考\n  ds suggest <ID> [消息]      分类型检查并建议目录结构，支持续接\n  ds chat <ID> <消息>         当前场景对话，建议保存到 proposal\n  ds merge <ID>              合并当前建议（请先通过 show 审查）\n  ds approve <ID>            确认审查所选操作，进入执行阶段\n  ds execute <ID>            执行已经确认的计划\n  ds rollback <ID>           恢复已完成的移动\n  ds trajectory <ID>         JSON 操作轨迹\n\nCtrl+C 停止长任务。配置：config.toml 或 .env；推荐通过网页设置页修改模型。\n旧版 trash/read/ask 入口已移除，没有删除命令。");
        return Ok(());
    }
    let (data, config_path) = ds_engine::config::runtime_paths()?;
    let config = AppConfig::load(&config_path)?;
    let store = TaskStore::new(data.join("tasks"))?;
    for mut task in store.list()? {
        safe_fs::recover(&mut task, &store)?;
        ds_engine::response_cache::recover(&store, &mut task)?;
    }
    store.recover_jobs()?;
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let progress = |done: usize, total: usize, message: &str| {
        eprintln!(
            "[{}] {message}",
            if total > 0 {
                format!("{done}/{total}")
            } else {
                done.to_string()
            }
        );
    };
    let arg = |index: usize| {
        args.get(index)
            .map(String::as_str)
            .context("参数不足，使用 ds help 查看用法")
    };
    if command == "list" {
        for task in store.list()? {
            println!(
                "{}  阶段 {}  {:14}  {}",
                task.id,
                task.phase,
                task.status,
                task.root.display()
            );
        }
        return Ok(());
    }
    if command == "create" {
        let mut task = Task::new(
            PathBuf::from(arg(1)?),
            args.get(2).map(String::as_str).unwrap_or("organize"),
            config.permissions,
        )?;
        store.save(&task)?;
        store.event(&task, "task_created", json!({"interface":"cli"}))?;
        let result = task.scan(&cancel, &progress);
        store.save(&task)?;
        result?;
        println!("{}", task.id);
        return Ok(());
    }
    if command == "archives" {
        println!("{}", serde_json::to_string_pretty(&store.list_archives()?)?);
        return Ok(());
    }
    if command == "archive-show" || command == "resume" {
        let id = uuid::Uuid::parse_str(arg(1)?)?;
        let archive = store.load_archive(id)?;
        if command == "archive-show" {
            println!("{}", serde_json::to_string_pretty(&archive)?);
        } else {
            let mut source = archive.task;
            if let Some(root) = args.get(2) {
                source.root = PathBuf::from(root);
            }
            let task = source.imported()?;
            store.save(&task)?;
            store.event(
                &task,
                "archive_resumed",
                json!({"archive_id":id,"interface":"cli"}),
            )?;
            println!("{}", task.id);
        }
        return Ok(());
    }
    if command == "import" {
        let value = ds_engine::archive::read_json(&PathBuf::from(arg(1)?))?;
        if value["format"] == "downloadsweeper-archive" {
            let id = store.import_archive(serde_json::from_value(value)?)?;
            println!("归档已只读保存：{id}");
        } else {
            let task: Task = serde_json::from_value(value)?;
            let task = task.imported()?;
            store.save(&task)?;
            store.event(&task, "task_imported", json!({"interface":"cli"}))?;
            println!("{}", task.id);
        }
        return Ok(());
    }
    let id = uuid::Uuid::parse_str(arg(1)?)?;
    let mut task = store.load(id)?;
    if matches!(command, "plan" | "suggest" | "chat" | "cleanup") {
        task.runtime_run = Some(uuid::Uuid::new_v4());
        task.touch();
        store.save(&task)?;
    }
    let result: Result<()> = async {
        match command {
            "show" => println!("{}", serde_json::to_string_pretty(&task)?),
            "trajectory" => println!("{}", serde_json::to_string_pretty(&store.events(id)?)?),
            "export" => safe_fs::atomic_json(
                &std::env::current_dir()?.join(arg(2)?),
                &store.export_archive(id)?,
            )?,
            "cleanup" => {
                ensure!(task.status == "completed", "请先完成整理");
                task.cleanup_options.validate()?;
                if args.iter().any(|s| s == "--ai") {
                    workflow_ai::cleanup_review(&mut task, &config, &store, &cancel, &progress)
                        .await?;
                } else {
                    task.prepare_cleanup();
                    task.touch();
                }
                println!("{}", serde_json::to_string_pretty(&task.cleanup)?);
            }
            "scan" => task.scan(&cancel, &progress)?,
            "next" => {
                let prepare_rules = task.phase == 2 && task.is_organizing();
                task.advance()?;
                if prepare_rules {
                    task.generate_rules(&cancel, &progress)?;
                }
            }
            "back" => {
                let phase = arg(2)?.parse::<u8>()?;
                task.go_back(phase)?;
            }
            "update" => {
                task.editable()?;
                let edited: Task = serde_json::from_slice(&std::fs::read(arg(2)?)?)?;
                ensure!(
                    edited.id == task.id && edited.revision == task.revision,
                    "编辑文件不属于当前任务或已过期"
                );
                let mut candidate = task.clone();
                match task.phase {
                    1 => {
                        candidate.permissions = edited.permissions;
                        candidate.validate_permissions()?;
                    }
                    2 => {
                        candidate.nodes = edited.nodes;
                        candidate.rename_extensions = edited.rename_extensions;
                        candidate.rename_web_search = edited.rename_web_search;
                        candidate.validate_graph(false)?;
                    }
                    4 => {
                        for op in &mut candidate.operations {
                            if let Some(other) = edited.operations.iter().find(|o| o.id == op.id) {
                                op.selected = other.selected;
                            }
                        }
                        candidate.reviewed = false;
                    }
                    _ => anyhow::bail!(
                        "仅能更新阶段 1 的权限、阶段 2 的树/命名范围或阶段 4 的操作选择"
                    ),
                }
                candidate.proposal = None;
                candidate.touch();
                task = candidate;
            }
            "plan" => {
                task.editable()?;
                ensure!(
                    task.phase == 3 || (task.phase == 4 && task.is_organizing()),
                    "请在生成计划或审查阶段运行"
                );
                if task.mode == "rename" {
                    workflow_ai::rename(&mut task, &config, &store, &cancel, &progress).await?;
                } else if args.iter().any(|a| a == "--ai") {
                    let options = workflow_ai::RefineOptions {
                        batch_size: args
                            .windows(2)
                            .find(|a| a[0] == "--batch-size")
                            .map(|a| a[1].parse())
                            .transpose()?
                            .unwrap_or(256),
                        thinking: args.iter().any(|a| a == "--thinking"),
                    };
                    workflow_ai::refine_with_options(
                        &mut task, &config, &store, options, &cancel, &progress,
                    )
                    .await?;
                } else {
                    task.generate_rules(&cancel, &progress)?;
                }
            }
            "suggest" => {
                workflow_ai::suggest_tree(
                    &mut task,
                    &config,
                    &store,
                    &if args.len() > 2 {
                        args[2..].join(" ")
                    } else {
                        "根据分类型检查结果建议目标目录结构".into()
                    },
                    &cancel,
                    &progress,
                )
                .await?;
            }
            "chat" => {
                let scene = task.scene().to_string();
                workflow_ai::chat(
                    &mut task,
                    &config,
                    &store,
                    &scene,
                    &args[2..].join(" "),
                    &cancel,
                    &progress,
                )
                .await?;
            }
            "merge" => {
                let p = task.proposal.clone().context("没有可合并建议")?;
                task.apply_proposal(
                    &p.id,
                    &p.changes.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
                    &p.scene,
                )?;
            }
            "approve" => {
                task.editable()?;
                ensure!(task.phase == 4, "请先生成并审查计划");
                task.reviewed = true;
                task.advance()?;
            }
            "execute" => safe_fs::execute(&mut task, &store, &cancel, &progress)?,
            "rollback" => safe_fs::rollback(&mut task, &store, &cancel, &progress)?,
            _ => anyhow::bail!("未知命令；使用 ds help 查看新流程"),
        }
        Ok(())
    }
    .await;
    store.save(&task)?;
    store.event(
        &task,
        command,
        json!({"interface":"cli","error":result.as_ref().err().map(ToString::to_string)}),
    )?;
    result?;
    if !["show", "trajectory", "export"].contains(&command) {
        println!(
            "任务 {} · 阶段 {} · {} · {} 项操作",
            task.id,
            task.phase,
            task.status,
            task.operations.len()
        );
    }
    Ok(())
}
