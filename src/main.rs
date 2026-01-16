mod git;
mod protocol;

use anyhow::Result;
use git::GitHandler;
use protocol::{CallToolParams, InitializeParams, JsonRpcRequest, Tool};

use serde_json::json;
use std::io::{self, BufRead, Write};

use std::sync::Mutex;

struct ServerConfig {
    commit_format: String,
}

lazy_static::lazy_static! {
    static ref CONFIG: Mutex<ServerConfig> = Mutex::new(ServerConfig {
        commit_format: r#"<type>[optional scope]: <english description>

[English body]

[Chinese body]

Log: [short description of the change use chinese language]
PMS: <BUG-number>(for bugfix) or <TASK-number>(for add feature) (Must include 'BUG-' or 'TASK-', If the user does not provide a number, remove this line.)
Influence: Explain in Chinese the potential impact of this submission."#.to_string(),
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    while let Some(Ok(line)) = lines.next() {
        eprintln!("收到请求: {}", line);
        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("JSON 解析失败: {}", e);
                continue;
            }
        };

        let is_notification = request.id.is_none();

        let response_payload = match request.method.as_str() {
            "initialize" => {
                if let Some(params_val) = &request.params {
                    if let Ok(params) =
                        serde_json::from_value::<InitializeParams>(params_val.clone())
                    {
                        if let Some(options) = params.options {
                            if let Some(format) =
                                options.get("commitFormat").and_then(|v| v.as_str())
                            {
                                let mut config = CONFIG.lock().unwrap();
                                config.commit_format = format.to_string();
                            }
                        }
                    }
                }

                Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "listChanged": true
                        }
                    },
                    "serverInfo": {
                        "name": "git-summarizer",
                        "version": "0.1.0"
                    }
                }))
            }
            "notifications/initialized" => {
                eprintln!("客户端已确认初始化");
                None
            }
            "tools/list" => {
                let config = CONFIG.lock().unwrap();
                let format_hint = config.commit_format.clone();
                let tools = vec![
                    Tool {
                        name: "get_staged_diff".to_string(),
                        description: format!(
                            "获取当前 git 暂存区的变更内容 (git diff --staged)。获取后，请你根据变更内容总结出一个提交信息，并询问用户是否提交。\n\n\
                            ### 提交格式要求：\n{}\n\n\
                            ### 额外约束：\n\
                            - Body 的每一行不得超过 80 个字符。\n\
                            - 如果修改范围很小，可以同时省略 English body 和 Chinese body。\n\
                            - 如果不省略 body，则必须同时保留 English body 和 Chinese body，不得只写其中一个。",
                            format_hint
                        ),
                        input_schema: json!({
                            "type": "object",
                            "properties": {}
                        }),
                    },

                    Tool {
                        name: "execute_commit".to_string(),
                        description: "执行提交。请在用户确认了你总结的提交信息后再调用此工具。".to_string(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {
                                "message": { "type": "string", "description": "提交信息" }
                            },
                            "required": ["message"]
                        }),
                    },
                ];
                Some(json!({ "tools": tools }))
            }
            "tools/call" => {
                let params: CallToolParams =
                    serde_json::from_value(request.params.clone().unwrap_or_default())?;
                let tool_result = match params.name.as_str() {
                    "get_staged_diff" => match GitHandler::get_staged_diff() {
                        Ok(diff) => json!({ "content": [{ "type": "text", "text": diff }] }),
                        Err(e) => {
                            json!({ "isError": true, "content": [{ "type": "text", "text": e.to_string() }] })
                        }
                    },
                    "execute_commit" => {
                        let arguments = params.arguments.as_ref();
                        let msg = arguments.and_then(|a| a["message"].as_str()).unwrap_or("");
                        match GitHandler::commit(msg) {
                            Ok(res) => json!({ "content": [{ "type": "text", "text": res }] }),
                            Err(e) => {
                                json!({ "isError": true, "content": [{ "type": "text", "text": e.to_string() }] })
                            }
                        }
                    }
                    _ => {
                        json!({ "isError": true, "content": [{ "type": "text", "text": "未知工具" }] })
                    }
                };
                Some(tool_result)
            }
            _ => {
                if is_notification {
                    None
                } else {
                    Some(json!({ "error": { "code": -32601, "message": "Method not found" } }))
                }
            }
        };

        if let (Some(payload), Some(id)) = (response_payload, request.id) {
            let mut response_obj = serde_json::Map::new();
            response_obj.insert("jsonrpc".to_string(), json!("2.0"));
            response_obj.insert("id".to_string(), id);

            if let Some(error) = payload.get("error") {
                response_obj.insert("error".to_string(), error.clone());
            } else {
                response_obj.insert("result".to_string(), payload);
            }

            let output = serde_json::to_string(&response_obj)?;
            println!("{}", output);
            io::stdout().flush()?;
            eprintln!("发送响应: {}", output);
        }
    }

    Ok(())
}
