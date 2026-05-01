# =====================================================================
# Agent template image for agent_service.
#
# Adapted from BigBIueWhale/vibe_web_terminal (commit 414b330 era), with
# the OpenCode / Mistral-Vibe agents removed and replaced with a single
# pinned Qwen Code 0.15.6 + a tmux+ttyd entrypoint.
#
# Build:
#   docker build \
#       -t qwen-agent-template:0.1.0 \
#       -f docker/Dockerfile \
#       docker/
#
# Then start agent_service (which `docker run`s this image per session).
# =====================================================================

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV TERM=xterm-256color
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

# ============================================================
# CORE UTILITIES — installed FIRST so we have curl + gnupg available
# for the NodeSource repo bootstrap below.
# ============================================================

RUN apt-get update && apt-get install -y \
    curl \
    wget \
    aria2 \
    vim \
    nano \
    emacs-nox \
    htop \
    tree \
    jq \
    yq \
    ripgrep \
    fd-find \
    bat \
    fzf \
    tmux \
    screen \
    zsh \
    sudo \
    openssh-client \
    openssh-server \
    ca-certificates \
    locales \
    man-db \
    less \
    file \
    unzip \
    zip \
    p7zip-full \
    xz-utils \
    bzip2 \
    rsync \
    strace \
    ltrace \
    lsof \
    psmisc \
    sysstat \
    patch \
    diffutils \
    colordiff \
    dos2unix \
    rename \
    pv \
    parallel \
    entr \
    at \
    cron \
    git \
    git-lfs \
    gitk \
    git-gui \
    git-svn \
    git-doc \
    tig \
    subversion \
    mercurial \
    socat \
    netcat-openbsd \
    iputils-ping \
    iputils-arping \
    iputils-tracepath \
    net-tools \
    dnsutils \
    iproute2 \
    traceroute \
    telnet \
    nmap \
    tcpdump \
    iftop \
    nethogs \
    mtr-tiny \
    whois \
    openssl \
    iperf3 \
    httpie \
    apt-transport-https \
    gnupg2 \
    wireguard-tools \
    openvpn \
    sshpass \
    sshfs \
    && rm -rf /var/lib/apt/lists/*

# ============================================================
# NODE 22 LTS via NodeSource — Qwen Code 0.15.6 needs Node >= 20.
# Ubuntu 24.04's stock nodejs is 18.x and is rejected by qwen at startup.
# We do this BEFORE any subsequent apt installs so the later sections
# pick up the NodeSource version.
# ============================================================

RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && \
    apt-get install -y nodejs && \
    node --version && \
    npm --version && \
    rm -rf /var/lib/apt/lists/*

# ============================================================
# BUILD TOOLCHAIN
# ============================================================
RUN apt-get update && apt-get install -y \
    build-essential \
    gcc \
    g++ \
    gfortran \
    cmake \
    cmake-curses-gui \
    make \
    automake \
    autoconf \
    libtool \
    pkg-config \
    nasm \
    yasm \
    swig \
    ccache \
    ninja-build \
    meson \
    libboost-dev \
    libboost-filesystem-dev \
    libboost-system-dev \
    libboost-thread-dev \
    libboost-program-options-dev \
    libboost-regex-dev \
    libboost-iostreams-dev \
    libboost-serialization-dev \
    libboost-date-time-dev \
    libboost-test-dev \
    libboost-log-dev \
    libboost-json-dev \
    gdb \
    gdbserver \
    gdb-multiarch \
    valgrind \
    clang \
    clang-format \
    clang-tidy \
    cppcheck \
    iwyu \
    binutils \
    binutils-multiarch \
    elfutils \
    dwarves \
    linux-tools-common \
    gcc-arm-none-eabi \
    gdb-arm-none-eabi \
    libnewlib-arm-none-eabi \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    openocd \
    minicom \
    picocom \
    u-boot-tools \
    device-tree-compiler \
    qemu-user-static \
    qemu-system-arm \
    && rm -rf /var/lib/apt/lists/*

# ============================================================
# BUILDROOT / YOCTO DEPS
# ============================================================
RUN apt-get update && apt-get install -y \
    bc \
    cpio \
    gawk \
    findutils \
    libncurses-dev \
    libncursesw5-dev \
    texinfo \
    bison \
    flex \
    gettext \
    libelf-dev \
    libmpfr-dev \
    libmpc-dev \
    libgmp-dev \
    libisl-dev \
    dialog \
    lzop \
    squashfs-tools \
    mtd-utils \
    genext2fs \
    e2fsprogs \
    dosfstools \
    mtools \
    fakeroot \
    diffstat \
    chrpath \
    python3-pexpect \
    python3-git \
    python3-jinja2 \
    python3-subunit \
    zstd \
    liblz4-tool \
    debianutils \
    libsdl2-dev \
    mesa-common-dev \
    libglu1-mesa-dev \
    xterm \
    gcc-multilib \
    g++-multilib \
    lib32z1-dev \
    libc6-dev-i386 \
    pylint \
    rpcsvc-proto \
    lz4 \
    && rm -rf /var/lib/apt/lists/*

# ============================================================
# NETWORK ANALYSIS + PYTHON + headless browser deps
# (Note: nodejs is intentionally absent here — already installed via
# NodeSource above. `npm` likewise — comes bundled with NodeSource node.)
# ============================================================
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
    libpcap-dev \
    tshark \
    wireshark-common \
    wireshark-doc \
    tcpreplay \
    tcpflow \
    ngrep \
    libnet1-dev \
    libnl-3-dev \
    libnl-genl-3-dev \
    libnl-route-3-dev \
    libnetfilter-queue-dev \
    libgeoip-dev \
    ethtool \
    ipset \
    conntrack \
    termshark \
    tcptrace \
    tcpstat \
    ssldump \
    tcpick \
    tcpxtract \
    chaosreader \
    foremost \
    httpry \
    netsniff-ng \
    hping3 \
    p0f \
    dsniff \
    sngrep \
    dnstop \
    nbtscan \
    httping \
    nfdump \
    argus-client \
    suricata \
    python3 \
    python3-pip \
    python3-venv \
    python3-dev \
    python3-setuptools \
    python3-wheel \
    xvfb \
    xauth \
    dbus \
    libx11-xcb1 \
    libxcomposite1 \
    libxcursor1 \
    libxdamage1 \
    libxi6 \
    libxtst6 \
    libxrandr2 \
    libasound2t64 \
    libatk1.0-0t64 \
    libatk-bridge2.0-0t64 \
    libcups2t64 \
    libdrm2 \
    libgbm1 \
    libgtk-3-0t64 \
    libnspr4 \
    libnss3 \
    libxss1 \
    libappindicator3-1 \
    fonts-liberation \
    xdg-utils \
    libu2f-udev \
    libvulkan1 \
    tidy \
    libxml2-utils \
    xmlstarlet \
    default-jre-headless \
    && rm -rf /var/lib/apt/lists/*

# ============================================================
# BUN
# ============================================================
RUN curl -fsSL https://bun.sh/install | BUN_INSTALL=/usr/local bash

# ============================================================
# GLOBAL NPM PACKAGES (browser automation, scraping, dev tools)
# ============================================================
RUN npm install -g --force \
    puppeteer \
    playwright \
    cheerio \
    jsdom \
    axios \
    node-fetch \
    got \
    undici \
    turndown \
    readability-cli \
    linkedom \
    dompurify \
    sanitize-html \
    html-minifier-terser \
    clean-css-cli \
    terser \
    uglify-js \
    js-beautify \
    prettier \
    eslint \
    typescript \
    ts-node \
    tsx \
    esbuild \
    webpack-cli \
    webpack \
    vite \
    rollup \
    babel-cli \
    nodemon \
    pm2 \
    express \
    fastify \
    zod \
    lodash \
    underscore \
    ramda \
    date-fns \
    moment \
    dayjs \
    uuid \
    nanoid \
    chalk \
    commander \
    yargs \
    inquirer \
    ora \
    boxen \
    sharp \
    jimp \
    pdf-lib \
    pdfkit \
    csv-parser \
    csv-stringify \
    xlsx \
    json5 \
    yaml \
    toml \
    dotenv \
    marked \
    markdown-it \
    highlight.js \
    mermaid \
    @mermaid-js/mermaid-cli \
    md-to-pdf \
    d3 \
    puppeteer-extra \
    puppeteer-extra-plugin-stealth \
    http-server \
    live-server \
    concurrently \
    cross-env \
    rimraf \
    glob \
    minimatch \
    semver \
    debug \
    fx \
    json-diff \
    json-server \
    serve \
    localtunnel \
    degit \
    svgo \
    npm-check-updates \
    depcheck \
    madge \
    license-checker \
    tree-sitter-cli \
    wappalyzer \
    tldr \
    speed-test \
    is-up-cli \
    public-ip-cli \
    internal-ip-cli \
    fkill-cli \
    empty-trash-cli \
    clipboard-cli \
    open-cli

# ============================================================
# IMAGE / PDF / CRYPTO / OCR / DOCUMENT LIBS
# ============================================================
RUN apt-get update && apt-get install -y \
    libjpeg-dev \
    libpng-dev \
    libtiff-dev \
    libwebp-dev \
    libopenjp2-7-dev \
    libfreetype6-dev \
    zlib1g-dev \
    libffi-dev \
    libssl-dev \
    libxml2-dev \
    libxslt1-dev \
    libcairo2-dev \
    libpango1.0-dev \
    libgdk-pixbuf2.0-dev \
    libglib2.0-dev \
    librsvg2-dev \
    libmagic1 \
    libmagickwand-dev \
    libicu-dev \
    icu-devtools \
    libfribidi-dev \
    libharfbuzz-dev \
    libsodium-dev \
    libgcrypt20-dev \
    libgnutls28-dev \
    libargon2-dev \
    libnss3-dev \
    libsasl2-dev \
    libkrb5-dev \
    libscrypt-dev \
    libsecret-1-dev \
    gnutls-bin \
    p11-kit \
    softhsm2 \
    tesseract-ocr \
    tesseract-ocr-heb \
    tesseract-ocr-ara \
    tesseract-ocr-eng \
    tesseract-ocr-rus \
    tesseract-ocr-fra \
    tesseract-ocr-deu \
    tesseract-ocr-spa \
    libtesseract-dev \
    leptonica-progs \
    poppler-utils \
    libpoppler-dev \
    libpoppler-cpp-dev \
    ghostscript \
    qpdf \
    pandoc \
    wkhtmltopdf \
    sqlite3 \
    libsqlite3-dev \
    postgresql-client \
    libpq-dev \
    redis-tools \
    liblapack-dev \
    libblas-dev \
    libopenblas-dev \
    libhdf5-dev \
    libgeos-dev \
    libproj-dev \
    libgdal-dev \
    && rm -rf /var/lib/apt/lists/*

# LibreOffice (writer + calc only)
RUN apt-get update && apt-get install -y --no-install-recommends \
    libreoffice-writer \
    libreoffice-calc \
    && rm -rf /var/lib/apt/lists/*

# ============================================================
# MULTIMEDIA + FONTS
# ============================================================
RUN apt-get update && apt-get install -y \
    ffmpeg \
    libavcodec-dev \
    libavformat-dev \
    libswscale-dev \
    libavutil-dev \
    libavfilter-dev \
    libavdevice-dev \
    imagemagick \
    sox \
    libsox-dev \
    libsox-fmt-all \
    mediainfo \
    libmediainfo-dev \
    libsndfile1-dev \
    vorbis-tools \
    lame \
    flac \
    opus-tools \
    mkvtoolnix \
    exiftool \
    webp \
    optipng \
    pngquant \
    jpegoptim \
    gifsicle \
    graphviz \
    libgraphviz-dev \
    gnuplot \
    fonts-dejavu \
    fonts-dejavu-core \
    fonts-dejavu-extra \
    fonts-liberation \
    fonts-liberation2 \
    fonts-noto \
    fonts-noto-color-emoji \
    fonts-noto-mono \
    fonts-noto-extra \
    fonts-noto-cjk \
    fonts-noto-cjk-extra \
    fonts-noto-ui-core \
    fonts-noto-ui-extra \
    fonts-freefont-ttf \
    fonts-opensymbol \
    fonts-open-sans \
    fonts-roboto \
    fonts-firacode \
    fonts-hack \
    culmus \
    culmus-fancy \
    fonts-hosny-amiri \
    fonts-arabeyes \
    fonts-kacst \
    fonts-kacst-one \
    fonts-droid-fallback \
    fonts-indic \
    fonts-thai-tlwg \
    fontconfig \
    fonts-symbola \
    fonts-powerline \
    fonts-font-awesome \
    fonts-ubuntu \
    fonts-ubuntu-console \
    fonts-cascadia-code \
    fonts-jetbrains-mono \
    fonts-lato \
    fonts-material-design-icons-iconfont \
    fonts-emojione \
    fonts-ancient-scripts \
    fonts-wine \
    fonts-ibm-plex \
    fonts-cantarell \
    fonts-crosextra-carlito \
    fonts-crosextra-caladea \
    fonts-nanum \
    fonts-nanum-coding \
    fonts-nanum-eco \
    fonts-nanum-extra \
    fonts-urw-base35 \
    fonts-sil-gentium-basic \
    fonts-sil-charis \
    fonts-wqy-microhei \
    fonts-wqy-zenhei \
    fonts-arphic-ukai \
    fonts-arphic-uming \
    fonts-ipafont-gothic \
    fonts-ipafont-mincho \
    fonts-vlgothic \
    fonts-takao-gothic \
    fonts-unfonts-core \
    fonts-unfonts-extra \
    fonts-sil-padauk \
    fonts-sil-abyssinica \
    fonts-lklug-sinhala \
    fonts-tibetan-machine \
    fonts-guru \
    fonts-guru-extra \
    fonts-lohit-guru \
    fonts-tlwg-garuda \
    fonts-tlwg-kinnari \
    fonts-tlwg-laksaman \
    fonts-tlwg-loma \
    fonts-tlwg-mono \
    fonts-tlwg-norasi \
    fonts-tlwg-purisa \
    fonts-tlwg-sawasdee \
    fonts-tlwg-typewriter \
    fonts-tlwg-typist \
    fonts-tlwg-typo \
    fonts-tlwg-umpush \
    fonts-tlwg-waree \
    xfonts-utils \
    && rm -rf /var/lib/apt/lists/* && fc-cache -fv

# PlantUML (uses graphviz + default-jre-headless from above)
RUN curl -fsSL -o /usr/local/lib/plantuml.jar \
      https://github.com/plantuml/plantuml/releases/download/v1.2025.0/plantuml-1.2025.0.jar \
    && printf '#!/bin/sh\nexec java -jar /usr/local/lib/plantuml.jar "$@"\n' > /usr/local/bin/plantuml \
    && chmod +x /usr/local/bin/plantuml

# ============================================================
# TERMINAL + CODECS + LANGUAGE PACKS
# ============================================================
RUN apt-get update && apt-get install -y \
    ncurses-term \
    libncursesw6 \
    ncurses-doc \
    libslang2-dev \
    libnotcurses-dev \
    notcurses-bin \
    libvterm-dev \
    libtermkey-dev \
    libtickit-dev \
    libcaca-dev \
    caca-utils \
    libaa1-dev \
    aalib1 \
    libnewt-dev \
    libreadline-dev \
    libedit-dev \
    libunistring-dev \
    libunibilium-dev \
    libutf8proc-dev \
    libfmt-dev \
    whiptail \
    figlet \
    toilet \
    toilet-fonts \
    cowsay \
    boxes \
    fortune-mod \
    cmatrix \
    sl \
    lolcat \
    libavcodec-extra \
    libx264-dev \
    libx265-dev \
    libvpx-dev \
    libopus-dev \
    libaom-dev \
    libdav1d-dev \
    libde265-dev \
    libtheora-dev \
    libspeex-dev \
    libmp3lame-dev \
    libogg-dev \
    libvorbis-dev \
    libflac-dev \
    libwavpack-dev \
    language-pack-en \
    language-pack-he \
    language-pack-ar \
    language-pack-zh-hans \
    language-pack-ja \
    language-pack-ko \
    language-pack-ru \
    language-pack-fr \
    language-pack-de \
    language-pack-es \
    language-pack-pt \
    language-pack-hi \
    language-pack-it \
    language-pack-pl \
    language-pack-tr \
    language-pack-uk \
    && rm -rf /var/lib/apt/lists/*

RUN locale-gen en_US.UTF-8 he_IL.UTF-8 ar_SA.UTF-8 ru_RU.UTF-8 fr_FR.UTF-8 \
    de_DE.UTF-8 es_ES.UTF-8 zh_CN.UTF-8 ja_JP.UTF-8 ko_KR.UTF-8 \
    hi_IN.UTF-8 pt_BR.UTF-8 it_IT.UTF-8 pl_PL.UTF-8 tr_TR.UTF-8 uk_UA.UTF-8

# ============================================================
# TERMINAL AGENT TOOLS
# ============================================================
RUN apt-get update && apt-get install -y \
    silversearcher-ag \
    cloc \
    shellcheck \
    shfmt \
    expect \
    moreutils \
    inotify-tools \
    procps \
    util-linux \
    coreutils \
    groff \
    wdiff \
    highlight \
    source-highlight \
    pigz \
    pbzip2 \
    pixz \
    asciinema \
    direnv \
    apache2-utils \
    miller \
    csvtool \
    xml-twig-tools \
    html-xml-utils \
    neovim \
    micro \
    fish \
    btop \
    mc \
    ranger \
    nnn \
    vifm \
    trash-cli \
    xclip \
    xsel \
    golang-go \
    rustc \
    cargo \
    ruby \
    ruby-dev \
    perl \
    lua5.4 \
    liblua5.4-dev \
    luarocks \
    lua-lpeg \
    lua-cjson \
    lua-socket \
    lua-filesystem \
    lua-sec \
    r-base-core \
    && rm -rf /var/lib/apt/lists/*

RUN luarocks install struct && \
    luarocks install lua-zlib && \
    luarocks install lua-messagepack && \
    luarocks install luabitop

# GitHub CLI
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | tee /etc/apt/sources.list.d/github-cli.list > /dev/null && \
    apt-get update && apt-get install -y gh && \
    rm -rf /var/lib/apt/lists/*

# glow (markdown reader)
RUN wget -qO /tmp/glow.deb https://github.com/charmbracelet/glow/releases/download/v2.0.0/glow_2.0.0_amd64.deb && dpkg -i /tmp/glow.deb && rm /tmp/glow.deb

# rclone
RUN curl -fsSL https://rclone.org/install.sh | bash

# Azure CLI
RUN curl -sL https://aka.ms/InstallAzureCLIDeb | bash && \
    rm -rf /var/lib/apt/lists/* && \
    az extension add --name azure-devops 2>/dev/null || true

# ============================================================
# REVERSE ENGINEERING / BINARY ANALYSIS / SECURITY / FORENSICS
# Tools an autonomous agent may need for security research, reverse
# engineering, malware analysis, and CTF-style work. The container has
# no internet access at run time, so all of these run on whatever the
# agent is given locally — pcaps, binaries, dumps, source.
# ============================================================
RUN apt-get update && apt-get install -y \
    radare2 \
    binwalk \
    hexedit \
    yara \
    clamav \
    john \
    hydra \
    aircrack-ng \
    sleuthkit \
    sqlmap \
    nikto \
    gobuster \
    dirb \
    wfuzz \
    scalpel \
    libcapstone-dev \
    libcapstone4 \
    && rm -rf /var/lib/apt/lists/*

# ============================================================
# PYTHON PACKAGES — split into many groups so a layer cache
# miss in one group doesn't invalidate the others.
# ============================================================

# Repair stale dist-info from system pip, then install build essentials.
RUN find /usr/lib/python3/dist-packages /usr/lib/python3.12/dist-packages \
        -name '*.dist-info' -type d ! -exec test -f '{}/RECORD' \; -exec rm -rf '{}' + 2>/dev/null; \
    pip3 install --break-system-packages --no-cache-dir setuptools wheel packaging charset-normalizer && \
    pip3 install --break-system-packages --no-cache-dir --upgrade pbr

RUN pip3 install --break-system-packages --no-cache-dir \
    numpy scipy pandas polars pyarrow "dask[complete]" "modin[dask]" xarray sympy statsmodels \
    more-itertools toolz cytoolz joblib orjson ujson pyyaml toml tomli tomli-w \
    python-dateutil arrow pendulum pytz chardet charset-normalizer tabulate prettytable tqdm \
    numba cython numexpr bottleneck swifter pandarallel \
    missingno ydata-profiling sweetviz pandera great-expectations \
    matplotlib seaborn plotly bokeh altair vega_datasets pygal wordcloud graphviz networkx pydot \
    plotnine holoviews hvplot datashader colorcet panel streamlit gradio nicegui

RUN pip3 install --break-system-packages --no-cache-dir \
    pypdf PyPDF2 pdfplumber "pdfminer.six" PyMuPDF pikepdf reportlab fpdf2 weasyprint xhtml2pdf \
    borb pdfrw img2pdf svglib "camelot-py[cv]" tabula-py pdf2image ocrmypdf pdf2docx \
    python-bidi arabic-reshaper PyICU pyfribidi hebrew-tokenizer camel-tools \
    azure-devops azure-cli-core azure-identity azure-storage-blob azure-storage-file-share msrest msal \
    openpyxl xlrd xlsxwriter xlwt odfpy python-docx python-pptx python-calamine mammoth docx2txt striprtf ebooklib csvkit petl \
    pypandoc markdownify html2text filetype python-magic xmltodict dicttoxml \
    mwclient mwparserfromhell wikitextparser

RUN pip3 install --break-system-packages --no-cache-dir \
    moviepy ffmpeg-python pydub librosa soundfile audioread imageio-ffmpeg \
    pytesseract Pillow opencv-python-headless scikit-image rawpy imageio Wand albumentations imgaug

# PyTorch CPU-only — installed before huggingface stack so the latter
# doesn't pull a 700MB GPU wheel by accident.
RUN pip3 install --break-system-packages --no-cache-dir \
    torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cpu

RUN pip3 install --break-system-packages --no-cache-dir --no-deps easyocr && \
    pip3 install --break-system-packages --no-cache-dir easyocr \
    rank-bm25 whoosh lunr faiss-cpu annoy hnswlib chromadb sentence-transformers txtai \
    openai anthropic huggingface-hub datasets transformers tokenizers sentencepiece tiktoken onnx onnxruntime \
    scikit-learn xgboost lightgbm catboost imbalanced-learn feature-engine category_encoders \
    optuna hyperopt shap lime eli5 mlflow

RUN pip3 install --break-system-packages --no-cache-dir \
    nltk spacy gensim textblob langdetect regex rapidfuzz "fuzzywuzzy[speedup]" ftfy unidecode python-Levenshtein polyglot wordfreq \
    pingouin lifelines arviz emcee uncertainties \
    requests "httpx[http2]" aiohttp beautifulsoup4 lxml cssselect html5lib scrapy parsel feedparser urllib3 \
    selenium playwright pyppeteer splinter mechanicalsoup grab httpcore

RUN pip3 install --break-system-packages --no-cache-dir \
    html5-parser bleach nh3 selectolax pyquery tinycss2 cssutils css-parser premailer \
    html-sanitizer htmlmin minify-html lxml-html-clean readability-lxml trafilatura newspaper3k boilerpy3 w3lib tldextract validators \
    sqlalchemy alembic psycopg2-binary asyncpg redis pymongo motor peewee dataset "databases[sqlite,postgresql]" duckdb lancedb \
    flask flask-cors flask-sqlalchemy flask-login flask-wtf flask-restful flask-socketio flask-talisman \
    django django-cors-headers djangorestframework fastapi "uvicorn[standard]" starlette gunicorn dash tornado \
    aiofiles websockets waitress hypercorn daphne twisted sanic falcon bottle cherrypy pyramid hug responder \
    werkzeug httptools h11 h2 wsproto uvloop python-socketio channels gevent eventlet pyopenssl trustme

RUN pip3 install --break-system-packages --no-cache-dir \
    click typer rich pydantic loguru structlog tenacity retry environs colorama termcolor blessed \
    prompt-toolkit questionary alive-progress yaspin humanize inflect natsort semver \
    textual textual-dev urwid npyscreen asciimatics pytermgui curtsies wcwidth emoji unicodedata2 \
    pyfiglet art asciichartpy plotext drawille colored blessings sty ansicolors colorful pastel termtables terminaltables \
    tox nox black ruff isort flake8 mypy pylint bandit safety vulture pyflakes autopep8 yapf pre-commit \
    pytest pytest-cov pytest-asyncio pytest-xdist pytest-mock pytest-benchmark hypothesis coverage faker factory-boy responses vcrpy freezegun time-machine \
    ipython ipdb pygments memory-profiler line-profiler scalene snakeviz py-spy objgraph pympler \
    jupyter jupyterlab notebook nbconvert nbformat ipywidgets ipykernel jupytext papermill nbstripout

RUN pip3 install --break-system-packages --no-cache-dir \
    cryptography pynacl pycryptodome pycryptodomex bcrypt argon2-cffi passlib hashids pyjwt "python-jose[cryptography]" \
    itsdangerous asn1crypto pyotp qrcode keyring paramiko fabric python-gnupg certifi cffi \
    truststore certvalidator oscrypto tls-parser sslyze tlslite-ng scrypt jwcrypto \
    shapely geopandas folium pyproj fiona rasterio geopy \
    boto3 google-cloud-storage python-dotenv python-multipart jinja2 mako markdown python-markdown-math celery dramatiq apscheduler watchdog luigi prefect \
    h5py tables fastparquet msgpack cbor2 protobuf flatbuffers lz4 zstandard brotli python-snappy avro fastavro \
    attrs cattrs dataclasses-json marshmallow cerberus dynaconf python-decouple omegaconf hydra-core typeguard beartype \
    sh plumbum invoke doit psutil supervisor gitpython pygithub dulwich \
    cachetools diskcache dogpile.cache aiocache trio anyio \
    boltons funcy pydash returns multipledispatch bidict sortedcontainers intervaltree bitarray mmh3 xxhash pybloom-live datasketch

RUN pip3 install --break-system-packages --no-cache-dir \
    yfinance pandas-ta prophet arch pmdarima tslearn sktime \
    scapy dpkt pyshark kamene pcapng python-pcapng impacket netaddr ipaddress netifaces nfstream \
    bpython ptpython xonsh jupyter-console litecli pgcli mycli iredis \
    glances ansible ansible-lint molecule \
    tldr howdoi thefuck cookiecutter copier cruft bump2version semantic-version twine build flit hatch pdm poetry \
    openapi-spec-validator jsonschema prance apispec flasgger datamodel-code-generator sqlacodegen \
    watchfiles inotify-simple deepdiff dictdiffer jsonpatch jsonpointer unidiff patch-ng \
    pexpect ptyprocess sarge delegator.py python-crontab schedule rq huey

# Reverse engineering / exploit dev / binary analysis Python tooling.
# (`angr` is intentionally excluded — it pulls ~1 GiB of deps and is
# overkill for what an agent typically needs; `z3-solver` covers the
# SMT side and `lief`/`pwntools`/`capstone` cover binary parsing.)
RUN pip3 install --break-system-packages --no-cache-dir \
    pwntools \
    volatility3 \
    capstone \
    unicorn \
    keystone-engine \
    yara-python \
    z3-solver \
    lief \
    ropper \
    ROPgadget

# NLTK data + spaCy small/medium English models
RUN python3 -c "import nltk; nltk.download('punkt'); nltk.download('punkt_tab'); nltk.download('stopwords'); nltk.download('wordnet'); nltk.download('averaged_perceptron_tagger'); nltk.download('averaged_perceptron_tagger_eng'); nltk.download('vader_lexicon'); nltk.download('maxent_ne_chunker'); nltk.download('maxent_ne_chunker_tab'); nltk.download('words'); nltk.download('omw-1.4'); nltk.download('universal_tagset')" && \
    PIP_BREAK_SYSTEM_PACKAGES=1 python3 -m spacy download en_core_web_sm && \
    PIP_BREAK_SYSTEM_PACKAGES=1 python3 -m spacy download en_core_web_md

# Playwright Chromium + Firefox (offline-ready)
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/playwright-browsers
RUN playwright install --with-deps chromium firefox && \
    npx playwright install chromium firefox && \
    chmod -R a+rX /opt/playwright-browsers

# Puppeteer reuses Playwright's Chromium (~400 MB saved)
ENV PUPPETEER_SKIP_CHROMIUM_DOWNLOAD=true
ENV PUPPETEER_EXECUTABLE_PATH=/opt/playwright-browsers/chromium-*/chrome-linux/chrome

# Convenience symlinks
RUN ln -sf /usr/bin/fdfind /usr/local/bin/fd && \
    ln -sf /usr/bin/batcat /usr/local/bin/bat

# ============================================================
# TTYD — built from source for OSC 52 clipboard support.
# Pinned to commit 9c87671 (xterm.js 5.5.0 + @xterm/addon-clipboard).
# ============================================================

RUN apt-get update && apt-get install -y \
    libjson-c-dev \
    libuv1-dev \
    && rm -rf /var/lib/apt/lists/*

# libwebsockets 4.3.6 with libuv (required by ttyd)
RUN git clone --depth 1 --branch v4.3.6 https://github.com/warmcat/libwebsockets.git /tmp/lws && \
    mkdir /tmp/lws/build && cd /tmp/lws/build && \
    cmake .. \
        -DLWS_WITH_LIBUV=ON \
        -DLWS_WITHOUT_CLIENT=ON \
        -DLWS_WITH_HTTP2=ON \
        -DCMAKE_BUILD_TYPE=Release && \
    make -j$(nproc) && make install && ldconfig && \
    rm -rf /tmp/lws

# ttyd: frontend (xterm.js + clipboard addon) then C backend
RUN git clone https://github.com/tsl0922/ttyd.git /tmp/ttyd && \
    cd /tmp/ttyd && git checkout 9c87671ccae9eefa3c01b08169272c0922e7cdff && \
    npm install -g corepack && corepack enable && \
    cd /tmp/ttyd/html && yarn install && yarn build && \
    mkdir /tmp/ttyd/build && cd /tmp/ttyd/build && \
    cmake .. -DCMAKE_BUILD_TYPE=Release && \
    make -j$(nproc) && \
    cp ttyd /usr/local/bin/ttyd && \
    rm -rf /tmp/ttyd

# uv (Python package manager)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:$PATH"

# ============================================================
# USER SETUP — agent (uid 1000) replacing Ubuntu's default `ubuntu` user.
# ============================================================

RUN userdel -r ubuntu 2>/dev/null || true \
    && useradd -m -s /bin/bash -G sudo -u 1000 agent \
    && echo "agent ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers \
    && mkdir -p /workspace /output /run/agent \
    && chown -R agent:agent /workspace /output \
    && chmod 0755 /workspace /output

# ============================================================
# QWEN CODE CLI — pinned to 0.15.6 (latest stable as of 2026-04-30,
# verified via `npm view @qwen-code/qwen-code dist-tags`).
# ============================================================

RUN npm install -g @qwen-code/qwen-code@0.15.6 \
    && qwen --version

# Bake config: settings.json (sampling params, modalities, yolo) and
# QWEN.md (operating instructions for the agent).
RUN mkdir -p /home/agent/.qwen && chown -R agent:agent /home/agent/.qwen
COPY --chown=agent:agent config/settings.json /home/agent/.qwen/settings.json
COPY --chown=agent:agent config/QWEN.md       /home/agent/.qwen/QWEN.md

# Container entrypoint scripts.
COPY config/agent_init.sh /usr/local/bin/agent_init.sh
COPY config/run_agent.sh  /usr/local/bin/run_agent.sh
RUN chmod 0755 /usr/local/bin/agent_init.sh /usr/local/bin/run_agent.sh

# A tiny manifest of the major versions, surfaced inside the agent's
# environment so the model can self-introspect what's installed.
RUN printf 'qwen-code: 0.15.6\nnode: %s\nnpm: %s\npython: %s\nttyd commit: 9c87671\n' \
      "$(node --version)" "$(npm --version)" "$(python3 --version)" \
    > /etc/agent-tools-manifest \
    && chmod 0644 /etc/agent-tools-manifest

# Disable telemetry / autoupdate / model-fetch network calls. Belt-and-braces:
# we already block all network egress to anything other than the proxy.
ENV QWEN_TELEMETRY_ENABLED=false
ENV QWEN_DISABLE_AUTOUPDATE=true
ENV QWEN_SANDBOX=false
ENV NO_COLOR=1

USER agent
WORKDIR /workspace

EXPOSE 7681

# Container's main process. agent_init.sh starts the agent in a detached
# tmux session and foregrounds ttyd so the host can wait for it.
CMD ["/usr/local/bin/agent_init.sh"]
