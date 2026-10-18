#!/usr/bin/env bash
# 在 ihomy 服务器上安装 Kada 更新镜像（root 执行）。
#
#   sudo ./install.sh
#
# 做三件事：装脚本到 /usr/local/bin/kada-mirror、装 systemd 单元、启用每日定时器并
# 立刻跑一次。依赖：curl、python3（Ubuntu 自带）。
set -euo pipefail
cd "$(dirname "$0")"

if [ "$(id -u)" -ne 0 ]; then
  echo "请用 root 执行：sudo $0" >&2
  exit 1
fi

install -m 755 mirror-kada.sh /usr/local/bin/kada-mirror
install -m 644 kada-mirror.service kada-mirror.timer /etc/systemd/system/

systemctl daemon-reload
systemctl enable --now kada-mirror.timer

echo "定时器已启用；现在立即同步一次（失败不影响定时器，下次会重试）："
systemctl start kada-mirror.service || true
sleep 2
systemctl --no-pager status kada-mirror.service || true
echo
echo "查看日志：journalctl -u kada-mirror.service -n 50 --no-pager"
echo "手动同步：sudo -u ihomy /usr/local/bin/kada-mirror"
