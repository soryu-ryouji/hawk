#!/usr/bin/env node
// 打洞探测口连通性检查（协议 P3）：向 hawk-server 探测口发一个无效 token 的探测包
// （定长 64B：HAWKP + 全零 token），期待否定应答 HAWK?（无效 token 专属，≤ 请求大小）。
//   收到 HAWK? → 端口可达且 hawk-server 在线（无效 token 是预期，无会话时必得此应答）
//   超时无响应 → UDP 被拦（安全组/防火墙未放行）或服务未运行
// 形状非法的包（非 64B/无 magic）探测口静默丢弃，手工 nc 发不出合法包——用本工具。
//
// 用法：node tools/check-probe-port.mjs <服务器地址> [端口=9101]
//   地址与 hawk-app 远程接入配置同一主机名即可（探测口不经反代，UDP 直达）

import dgram from "node:dgram";

const host = process.argv[2];
const port = Number(process.argv[3] ?? 9101);
if (!host) {
    console.error("用法: node tools/check-probe-port.mjs <服务器地址> [端口=9101]");
    process.exit(2);
}

const packet = Buffer.alloc(64);
packet.write("HAWKP", 0); // 全零 token：必然无效，专测连通性

const socket = dgram.createSocket("udp4");
const timer = setTimeout(() => {
    console.error(`✗ ${host}:${port} 3s 无响应：UDP 被拦（检查云安全组/防火墙的 UDP ${port}）或 hawk-server 未运行`);
    socket.close();
    process.exit(1);
}, 3000);

socket.on("message", (msg, rinfo) => {
    clearTimeout(timer);
    const magic = msg.subarray(0, 5).toString("latin1");
    if (magic === "HAWK?" || magic === "HAWKR") {
        console.log(`✓ 探测口可达：${rinfo.address}:${rinfo.port} 应答 ${magic}（hawk-server 在线）`);
    } else {
        console.log(`? 收到未知应答（${msg.length}B）：${msg.toString("hex")}`);
    }
    socket.close();
});
socket.on("error", (e) => {
    clearTimeout(timer);
    console.error(`✗ 发送失败: ${e.message}`);
    process.exit(1);
});
socket.send(packet, port, host);
