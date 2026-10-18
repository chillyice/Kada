# Kada 更新镜像（GitHub → ihomy）

应用内「检查更新」默认读 **ihomy 镜像** 的清单（国内可直连），能直连 GitHub 的人在设置页
可切到官方 Releases。本目录是镜像侧的全部内容：服务器每天把 GitHub 上**已发布**的最新
Release 同步到 `/opt/ihomy/uploads/kada/updates/`，客户端从
`https://ihomy.top/files/kada/updates/latest.json` 取清单。

## 信任模型

- 镜像**只搬文件、改 URL**：`latest.json` 里的 `signature` 原样保留，签名签的是安装包
  本身；客户端仍用内嵌公钥做 Ed25519 校验。镜像改包会验签失败，镜像也无法伪造签名。
- GitHub 的 `/releases/latest` 天然排除 draft / prerelease——Release 草稿不会被镜像出去，
  与客户端选 GitHub 源时的口径一致。
- 先落安装包与 `.sig`，最后原子替换 `latest.json`；客户端不会拿到指向缺失文件的清单。

## 安装（ihomy 服务器，root）

依赖：`curl`、`python3`（Ubuntu 自带）。先确认 `/opt/ihomy/uploads/kada/` 归 `ihomy` 所有。

```bash
cd scripts/ihomy-mirror
sudo ./install.sh
```

`install.sh` 会：装 `kada-mirror` 到 `/usr/local/bin/`、装 systemd 单元、启用每日定时器
（04:17 起随机错开 20 分钟，停机错过会补跑），并立即同步一次。

## 验证

```bash
# 清单能匿名取到，且版本 / 平台条目正确
curl -s https://ihomy.top/files/kada/updates/latest.json | python3 -m json.tool | head -20

# 清单指向的文件在 ihomy 上可下载（大小与 GitHub 一致）
curl -sI https://ihomy.top/files/kada/updates/latest.json

# 最近一次同步
journalctl -u kada-mirror.service -n 50 --no-pager
systemctl list-timers kada-mirror.timer
```

## 手动操作

```bash
# 立即同步一次（以文件属主身份跑，别用 root）
sudo -u ihomy /usr/local/bin/kada-mirror

# 强制重拉当前 tag（忽略 .synced-tag）
sudo -u ihomy env KADA_MIRROR_FORCE=1 /usr/local/bin/kada-mirror

# 换落盘目录 / 对外地址（一般只在测试时用）
KADA_MIRROR_DEST=/tmp/kada-updates KADA_MIRROR_PUBLIC_BASE=https://example.test/x \
  ./mirror-kada.sh
```

## 本地测试（不需要真 GitHub）

`KADA_MIRROR_API` 可指向本地假 API：把一份 Release JSON 与其中的资产放在
`python3 -m http.server` 后面，再用 `KADA_MIRROR_DEST` / `KADA_MIRROR_PUBLIC_BASE`
指到临时目录即可端到端验证「下载 → 改写清单 → 原子替换 → 第二次跳过」。

## 日志与失败处理

- 网络不通、GitHub 限流（403）、清单缺签名：脚本非零退出，journal 里有具体原因；定时器
  第二天会重试。**已同步的旧清单不会被破坏**（新文件全部下载成功后才替换）。
- 磁盘只增不减：每个版本保留自己的安装包（客户端可能还在下旧版本）。版本文件按
  `Kada_<版本>_<平台>...` 命名，天然不会互相覆盖。
