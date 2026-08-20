FROM mcr.microsoft.com/devcontainers/base:ubuntu-24.04

# Links the published image to this repository on GHCR. It is what puts a source
# link on the package page and lets this repository's Actions push to the
# package; see .github/workflows/devcontainer-prebuild.yml. It does not make the
# package public -- that is a one-time setting on the package itself.
LABEL org.opencontainers.image.source="https://github.com/blooop/devlaunch"

# The pixi that has to read `pixi.lock` as committed, which is what pins this
# rather than "latest at the time somebody bumped it".
#
# `pixi.lock` has been `version: 7` since 4dd28c8 (2026-08-15), written by the
# pixi a developer runs on the host (0.77.0 here). v0.63.1 reads a maximum of
# lock version 6, so every container create since then has hit:
#
#   warn ⚠ Lock-file version 7 is newer than supported
#   warn help: Maximum supported version: 6 (pixi v0.63.1)
#   warn The lock-file will be treated as missing and regenerated.
#
# That last line is the cost, and it is not a warning: the container throws the
# committed resolution away and solves its own. So the environment in there is
# not the locked one -- the reproducibility `frozen: true` buys CI is absent
# exactly where someone is doing the work -- the create pays a full solve every
# time, and `pixi.lock` is tracked, so the regenerated v6 file leaves the work
# tree dirty. Commit that by accident and the break inverts: the host's pixi is
# then the one that cannot read the lock.
#
# 0.77.0 rather than 0.68.0, which measurement says is the *minimum* that reads
# v7 (0.67.0 cannot; the boundary was probed one version at a time against this
# very lockfile). The minimum would work today and re-open the same gap
# immediately, because the gap is the container's pixi lagging the pixi that
# writes the lock. Matching the writer closes it, and 0.77.0 is also the
# better-tested choice: it is what already runs against this project on the host.
ARG PIXI_VERSION=v0.77.0

RUN curl -L -o /usr/local/bin/pixi -fsSL --compressed "https://github.com/prefix-dev/pixi/releases/download/${PIXI_VERSION}/pixi-$(uname -m)-unknown-linux-musl" \
    && chmod +x /usr/local/bin/pixi \
    && pixi info

# set some user and workdir settings to work nicely with vscode
USER vscode
WORKDIR /home/vscode

RUN echo 'eval "$(pixi completion -s bash)"' >> /home/vscode/.bashrc \
    && echo 'export PATH="$HOME/.pixi/bin:$PATH"' >> /home/vscode/.profile \
    && echo '# Workaround: pixi trampoline fails for bash scripts, so add env bin directly' >> /home/vscode/.profile \
    && echo '[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"' >> /home/vscode/.profile

# Somewhere for the two mounted ssh files to land, with the permissions ssh
# insists on. The directory around them is deliberately not mounted: what devpod
# writes into it in here has no meaning anywhere else, so it should not outlive
# the container.
RUN mkdir -p /home/vscode/.ssh && chmod 700 /home/vscode/.ssh

# Create .config/gh directory for GitHub CLI config mounts
RUN mkdir -p /home/vscode/.config/gh
