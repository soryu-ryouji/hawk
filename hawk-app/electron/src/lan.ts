// 本机局域网 IPv4 地址列表（设置面板展示访问地址用；[web] 配置读写由前端直连 daemon REST app/lan）。
import os from 'node:os';

export function lanAddresses(): string[] {
  const addresses: string[] = [];
  for (const list of Object.values(os.networkInterfaces())) {
    for (const item of list ?? []) {
      // 运行时 Node 18+ family 为数字 4/6（@types/node 声明为字符串字面量联合，与实际运行值不符）
      const family = item.family as unknown as string | number;
      if ((family === 4 || family === 'IPv4') && !item.internal) addresses.push(item.address);
    }
  }
  return addresses;
}
