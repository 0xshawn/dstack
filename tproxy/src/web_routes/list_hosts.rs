use crate::main_service::AppState;
use crate::main_service::RpcHandler;
use ra_rpc::RpcCall;
use rocket::{response::content::RawHtml as Html, State};
use tproxy_rpc::tproxy_server::TproxyRpc;

pub async fn list_hosts(state: &State<AppState>) -> Html<String> {
    let rpc_handler = RpcHandler::construct(state, None).expect("Failed to construct RpcHandler");
    let response = rpc_handler.list().await.expect("Failed to list hosts");

    let mut html = String::from(
        r#"
        <html>
        <head>
            <style>
                body { font-family: Arial, sans-serif; margin: 0; padding: 20px; background-color: #f0f0f0; }
                h1 { color: #333; }
                table { width: 100%; border-collapse: collapse; background-color: white; }
                th, td { padding: 10px; text-align: left; border-bottom: 1px solid #ddd; }
                th { background-color: #4CAF50; color: white; }
                tr:hover { background-color: #f5f5f5; }
                a { color: #1a73e8; text-decoration: none; }
                a:hover { text-decoration: underline; }
            </style>
        </head>
        <body>
            <h1>CVM Hosts</h1>
            <table>
                <tr>
                    <th>App ID</th>
                    <th>IP</th>
                    <th>Endpoint</th>
                </tr>
    "#,
    );

    for host in response.hosts {
        html.push_str(&format!(
            r#"
                <tr>
                    <td>{}</td>
                    <td>{}</td>
                    <td><a href="{}" target="_blank">{}</a></td>
                </tr>
        "#,
            host.app_id, host.ip, host.endpoint, host.endpoint
        ));
    }

    html.push_str(
        "
            </table>
        </body>
        </html>
    ",
    );

    Html(html)
}
