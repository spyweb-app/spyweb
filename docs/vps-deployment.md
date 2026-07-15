# VPS Setup & Deployment Guide

This guide covers everything you need to go from a fresh Linux VPS to a working SpyWeb monitoring station.

---

## 1. Installation

Download the latest Linux binary package directly to your server.

```bash
# Download and extract SpyWeb
curl -L -o spyweb.tar.gz https://dl.spyweb.app/linux && tar -xf spyweb.tar.gz && rm spyweb.tar.gz

# Enter the directory and remove the tray version (not needed on VPS)
cd spyweb && rm spyweb-tray

# Set executable permissions (if needed)
chmod +x spyweb
```

> **Note:** The release package comes pre-bundled with the `ui/` dashboard and a `jobs/starter-kit/` directory, so you are ready to run immediately after extracting.

---

## 2. Running in the Background

On a VPS, you want SpyWeb to stay alive even if the server reboots or the process crashes. The most lightweight and reliable way to do this is using a **systemd service**.

### Create the Service File
Use Helix (the recommended editor below) to create and edit the service file:

```bash
sudo hx /etc/systemd/system/spyweb.service
```

Paste the following content into the file:
1. Press **`i`** to enter **Insert Mode**.
2. Paste using **`Ctrl+Shift+V`** (most terminals) or **Right-Click → Paste**.
3. Press **`Esc`** when finished, then type **`:x`** (save and exit) and hit **Enter**.

```ini
[Unit]
Description=SpyWeb Scraper Service
After=network.target

[Service]
Type=simple
# For better security, create a dedicated user:
# sudo useradd -r -s /bin/false spyweb
# Then change User=root to User=spyweb
# And update WorkingDirectory/ExecStart paths accordingly
User=root
# Change this to the actual path where you installed spyweb
WorkingDirectory=/root/spyweb
ExecStart=/root/spyweb/spyweb start
Restart=always
RestartSec=5

# Optional: Override the default port (default: 7979)
# ExecStart=/root/spyweb/spyweb start --port 8080
# Environment=SPYWEB_PORT=8080

# Optional: Disable the web server entirely (scraping only)
# Environment=SPYWEB_DISABLE_SERVER=1

[Install]
WantedBy=multi-user.target
```

### Manage the Service
Once the file is saved, run these commands to start and enable it:

```bash
# Reload systemd to pick up the new file
sudo systemctl daemon-reload

# Enable it to start automatically on boot
sudo systemctl enable spyweb

# Start it now
sudo systemctl start spyweb

# Check status
sudo systemctl status spyweb
```

### How to see logs?
Since SpyWeb is now a service, you can view the live logs using `journalctl`:
```bash
sudo journalctl -u spyweb -f
```

---

## 3. Token Validation (Recommended)

Protect your API endpoints with a secret key. When set, all requests to the built-in API server must include the key in the `X-SpyWeb-Key` header.

```bash
# Set in systemd service file
Environment=SPYWEB_API_KEY=your-secret-key-here
```

Or export directly:

```bash
export SPYWEB_API_KEY=your-secret-key-here
```

Test with curl:

```bash
curl -H "X-SpyWeb-Key: your-secret-key-here" http://localhost:7979/api/jobs
```

Without the correct header, requests return `401 Unauthorized`.

---

## 4. Remote Editor Setup (Optional)

For setups with complex Lua hooks and frequent configuration edits, configuring a remote editor with Language Server Protocol (LSP) integration is highly recommended. It provides an IDE-like experience directly on the VPS, ensuring that syntax errors in your scripts or TOML configs are caught immediately.

### A. Helix Editor
A modern, modal terminal editor with built-in LSP support and no editor configuration required—Helix auto-detects installed language servers automatically.

```bash
# Ubuntu/Debian
sudo apt update && sudo apt install helix

# For older versions, you might need the PPA:
# sudo add-apt-repository ppa:maveonair/helix-editor && sudo apt update

# Arch Linux
sudo pacman -S helix

# Fedora
sudo dnf install helix
```

> [!TIP]
> **Don't get stuck!** Like Vim, Helix is a "modal" editor (but with mouse support included):
> - Press **`i`** to start typing (**Insert mode**).
> - Press **`Esc`** to stop typing (**Normal mode**).
> - **To Save (write):** While in Normal mode, type **`:w`** then Enter.
> - **To Exit (quit):** While in Normal mode, type **`:q`** then Enter.
> - **To Exit without saving:** While in Normal mode, type **`:q!`** then Enter.
>
> **Mnemonic:** Just remember **w** = **w**rite and **q** = **q**uit.

### B. lua-language-server
Provides real-time syntax checking for your `hooks.lua` scripts. It will show you a red underline if you forget an `end` or mistype a function.

```bash
# Ubuntu/Debian
sudo apt install lua-language-server

# Arch Linux
sudo pacman -S lua-language-server

# Fedora
sudo dnf install lua-language-server

# Manual (Prebuilt Binary)
# Download from: https://github.com/LuaLS/lua-language-server/releases
```

### C. Taplo (TOML LSP)
Validates your `jobs.toml` and `config.toml` files.

```bash
# Download the prebuilt "full" binary (LSP + CLI)
wget https://github.com/tamasfe/taplo/releases/latest/download/taplo-full-linux-x86_64.gz

# Extract and install
gunzip taplo-full-linux-x86_64.gz
chmod +x taplo-full-linux-x86_64
sudo mv taplo-full-linux-x86_64 /usr/local/bin/taplo
```
### Why use this setup?
Because SpyWeb supports hot-reloading, saving any configuration or Lua script will instantly reload the job. Using an LSP is beneficial for two reasons:
1. **Immediate Syntax Validation:** If a Lua script contains a syntax error, the engine logs the error and continues the job by bypassing the broken hook stage. An LSP ensures you catch typos before reloading.
2. **Autocompletion:** Simplifies writing custom hooks by showing available variables and APIs.

---

## 5. Accessing the UI (Securely)
By default, SpyWeb serves the admin dashboard on port **7979**. While you can open this port globally, we highly recommend one of the following secure access methods:

### Option 1: SSH Tunnel (Zero Public Exposure)
This is the **safest** method. You don't open any ports on the VPS; instead, you "tunnel" the traffic through your existing SSH connection.

```bash
# Run this on your LOCAL machine (Laptop/Desktop), not the VPS
ssh -L 7979:localhost:7979 user@your-vps-ip
```

Then, simply open `http://localhost:7979` in your local browser. The data travels through SSH, and the port remains closed to the outside world.

### Option 2: Nginx Reverse Proxy

Use this if you want a custom domain and shared access.

First, [set up SPYWEB_API_KEY](#3-token-validation-recommended) as your primary auth layer. All API requests will require the `X-SpyWeb-Key` header.

```bash
# Install Nginx
sudo apt install nginx
```

Then create an Nginx config (e.g., `/etc/nginx/sites-available/spyweb`):

```nginx
server {
    listen 80;
    server_name your-domain.com;

    location / {
        proxy_pass http://127.0.0.1:7979;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

**Optional: Add Basic Auth** for an extra password prompt before reaching the dashboard:

```bash
sudo apt install apache2-utils
sudo htpasswd -c /etc/nginx/.htpasswd yourusername
```

Add `auth_basic` lines to the `location /` block:

```nginx
    location / {
        auth_basic "SpyWeb Admin";
        auth_basic_user_file /etc/nginx/.htpasswd;
        proxy_pass http://127.0.0.1:7979;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
```

### Option 3: Caddy (Automatic HTTPS)

Zero-config HTTPS via Let's Encrypt. Caddy automatically provisions and renews certificates.

```bash
# Install Caddy
sudo apt install caddy
```

Caddyfile (`/etc/caddy/Caddyfile`):

```caddy
your-domain.com {
    reverse_proxy localhost:7979
}
```

**Optional: Add Basic Auth** by adding a `basicauth` directive:

```caddy
your-domain.com {
    basicauth {
        yourusername $2a$14$hash  # generate with: caddy hash-password
    }
    reverse_proxy localhost:7979
}
```

### Option 4: UFW Whitelist (Your IP Only)
If you have a static IP at home/office, you can tell the VPS firewall to only talk to you.

```bash
# Replace YOUR_HOME_IP with your actual public IP
sudo ufw allow from YOUR_HOME_IP to any port 7979
```

This blocks the entire world while letting you through seamlessly.
