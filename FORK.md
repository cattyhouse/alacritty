# 在 origin（上游）和 myfork（自己的 fork）之间干活

远端一览：

- `origin` = `alacritty/alacritty` 上游，只读。永远不要往这里 push，没有权限，也不需要。
- `myfork` = `cattyhouse/alacritty` 你自己的 fork，可写。所有提交都推到这里。

看远端和分支：

```sh
git remote -v
git fetch --all --prune
git branch -vv
```

## 在两个远端之间切换

所谓“切换”，就是决定从哪边取代码、往哪边推代码：

- 从上游取最新：`git fetch origin`
- 从 fork 取最新：`git fetch myfork`
- 基于 fork 的分支开干：`git checkout --track myfork/fix`（第一次），之后直接 `git checkout fix`
- 推送时指明远端：`git push myfork fix`，想省事就设默认上游：`git branch --set-upstream-to=myfork/fix fix`，之后直接 `git push`
- 看某分支两边差多少：`git log --oneline origin/master..HEAD`（上游有啥我没有），`git log --oneline myfork/master..HEAD`（fork 有啥我没有）

## 上游 origin 有了新 commit，同步过来

推荐 rebase，保持自己的补丁永远在最上面，历史是直线：

```sh
git fetch origin
git checkout master
git rebase origin/master
```

有冲突就按提示解完 `git add` 对应文件，然后 `git rebase --continue`。不想要了就 `git rebase --abort`。

同步完推到自己的 fork（rebase 改写了历史，第一次推加 `--force-with-lease`，安全版强制推送）：

```sh
git push myfork master --force-with-lease
```

不想 rebase 也行，用 merge，多一个合并节点：

```sh
git fetch origin
git checkout master
git merge origin/master
git push myfork master
```

## 给 fix 这类补丁分支同步上游

```sh
git fetch origin
git checkout fix
git rebase origin/master
git push myfork fix --force-with-lease
```

## 本仓库分支说明

- `master`：上游 base 之上的个人补丁（CJK 单 atlas、XTMODKEYS 兜底、Ctrl+Shift+字母区分），构建 `/Applications/Alacritty.app` 就用它。
- `fix`：推给别人看 / 提 PR 用的同内容分支。
- `myfork/master`：保持可快进，不要在这里直接改东西，所有改动先在本地分支做完再推。
