# Kada 更新镜像与卡带市场（GitHub → ihomy）

应用内「检查更新」默认读 **ihomy 镜像** 的清单（国内可直连），能直连 GitHub 的人在设置页
可切到官方 Releases。本目录是镜像侧的全部内容：服务器每天把 GitHub 上**已发布**的最新
Release 同步到 `/opt/ihomy/uploads/kada/updates/`，客户端从
`https://ihomy.top/files/kada/updates/latest.json` 取清单。

同一趟还会同步**卡带市场**：仓库 `snippets/` 下的索引与卡带文件落到
`/opt/ihomy/uploads/kada/snippets/`，客户端默认从
`https://ihomy.top/files/kada/snippets/index.json` 取索引。索引里的条目地址会被改写成
ihomy 的地址（客户端只认条目里的 `url`；原地址是 GitHub raw，国内会卡住）。
卡带每天全量重拉一遍，源文件只有几 KB。

## 信任模型

- 镜像**只搬文件、改 URL**：`latest.json` 里的 `signature` 原样保留，签名签的是安装包
  本身；客户端仍用内嵌公钥做 Ed25519 校验。镜像改包会验签失败，镜像也无法伪造签名。
- GitHub 的 `/releases/latest` 天然排除 draft / prerelease——Release 草稿不会被镜像出去，
  与客户端选 GitHub 源时的口径一致。
- 先落安装包与 `.sig`，最后原子替换 `latest.json`；客户端不会拿到指向缺失文件的清单。
- 卡带同理：只改索引条目里的 `url`，卡带内容原样搬运；先落卡带文件、最后替换
  `index.json`。**卡带与更新清单互不牵连**——卡带同步失败只在 journal 记一行，当天的
  更新照常同步。

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

# 卡带索引能匿名取到，条目地址都指向 ihomy（不应该再出现 raw.githubusercontent.com）
curl -s https://ihomy.top/files/kada/snippets/index.json | python3 -m json.tool | head -30
curl -s https://ihomy.top/files/kada/snippets/index.json | grep -c raw.githubusercontent.com  # 期望 0

# 随便取一份卡带，能当 Config 解析
curl -s https://ihomy.top/files/kada/snippets/phrases.json | python3 -m json.tool | head -5

# 最近一次同步
journalctl -u kada-mirror.service -n 50 --no-pager
systemctl list-timers kada-mirror.timer
```

## 手动操作

```bash
# 立即同步一次（以文件属主身份跑，别用 root）
sudo -u ihomy /usr/local/bin/kada-mirror

# 强制重拉当前 tag（忽略 .synced-tag；卡带本来就每天全量重拉）
sudo -u ihomy env KADA_MIRROR_FORCE=1 /usr/local/bin/kada-mirror

# 换落盘目录 / 对外地址（一般只在测试时用）
KADA_MIRROR_DEST=/tmp/kada-updates KADA_MIRROR_PUBLIC_BASE=https://example.test/x \
KADA_MIRROR_SNIPPET_DEST=/tmp/kada-snippets \
KADA_MIRROR_SNIPPET_PUBLIC_BASE=https://example.test/snippets \
  ./mirror-kada.sh
```

## 下载走代理（服务器直连 GitHub 资产很慢时）

ihomy 服务器到 `github.com/releases/download` 只有几十 KB/s（甚至挂住），直接同步 80MB 的
AppImage 要一个多小时。unit 里已带 `KADA_MIRROR_GH_BASE=https://ghfast.top/`，脚本会把每个
资产 URL 前面拼上这个前缀（`api.github.com` 的 Release 元数据仍直连）。代理只搬运字节、
不改签名，客户端照样验签。卡带文件（`raw.githubusercontent.com`）也走同一个前缀。代理挂了
就把这项 `Environment=` 删掉回落直连。

## 本地测试（不需要真 GitHub）

`KADA_MIRROR_API` 可指向本地假 API：把一份 Release JSON 与其中的资产放在
`python3 -m http.server` 后面，再用 `KADA_MIRROR_DEST` / `KADA_MIRROR_PUBLIC_BASE`
指到临时目录即可端到端验证「下载 → 改写清单 → 原子替换 → 第二次跳过」。

卡带侧同理：把仓库 `snippets/` 目录放到本地静态服务后面，用 `KADA_MIRROR_RAW_BASE`
指向它、`KADA_MIRROR_SNIPPET_DEST` 指到临时目录，跑一次应看到索引里的 `url` 全部改写
成 `KADA_MIRROR_SNIPPET_PUBLIC_BASE`。

## 日志与失败处理

- 网络不通、GitHub 限流（403）、清单缺签名：脚本非零退出，journal 里有具体原因；定时器
  第二天会重试。**已同步的旧清单不会被破坏**（新文件全部下载成功后才替换）。
- 卡带索引版本不是 1、文件名非法 / 重复、某份卡带不是合法 JSON 或缺元信息：**整批不落盘**，
  旧索引与旧卡带继续可用，journal 里记具体原因。
- 磁盘只增不减：每个版本保留自己的安装包（客户端可能还在下旧版本）。版本文件按
  `Kada_<版本>_<平台>...` 命名，天然不会互相覆盖；卡带从索引里撤下后旧文件也不删
  （删了会让缓存里的旧索引指向 404）。
