// 本机局域网 IPv4 地址列表（设置面板展示访问地址用；[web] 配置读写由前端直连 daemon REST app/lan）。
import os from 'node:os';

export function lanAddresses() {
  const addresses = [];
  for (const list of Object.values(os.networkInterfaces())) {
    for (const item of list ?? []) {
      // Node 18+ family 为数字 4/6（旧版本为字符串 'IPv4'/'IPv6'），两种都兼容
      const isIpv4 = item.family === 4 || item.family === 'IPv4';
      if (isIpv4 && !item.internal) addresses.push(item.address);
    }
  }
  return addresses;
}
