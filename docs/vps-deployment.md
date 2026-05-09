# VPS Setup & Deployment Guide

This guide covers everything you need to go from a fresh Linux VPS to a production-ready SpyWeb monitoring station.

---

## 1. Installation

Download the latest Linux binary package directly to your server.

```bash
# Download and extract SpyWeb
curl -L -o spyweb.zip https://dl.spyweb.app/linux && tar -xf spyweb.zip && rm spyweb.zip

# Enter the directory and remove the tray version (not needed on VPS)
cd spyweb && rm spyweb-tray

# Set executable permissions (if needed)
chmod +x spyweb
```

> **Note:** The release package comes pre-bundled with the `ui/` dashboard and a `jobs/starter-kit/` directory, so you are ready to run immediately after extracting.

---

## 2. Running in the Background

On a VPS, you want SpyWeb to stay alive even if the server reboots or the process crashes. The most lightweight and "bullet-proof" way to do this is using a **systemd service**.

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

## 3. Remote Editor Setup (Recommended for complex setup with heavy lua logic)

If you are building complex monitoring stations with **heavy Lua logic** and frequent updates, setting up a proper remote environment is a game-changer. I personally recommend the **Helix Editor** + **LSP** workflow because it gives you an IDE-like experience on a remote VPS.

This setup ensures that you catch errors in your **hooks and configs** before you even hit save.

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
SpyWeb features **Hot Reloading**. When you save a file (`.lua`, `.toml`), SpyWeb instantly detects the change and attempts to respawn the job.

*   **For Power Users:** Using LSPs prevents **"Hook Bypassing."** If your Lua script has a syntax error, SpyWeb will log the error but *continue* the job by skipping the broken hook. Without LSPs, your custom filters or transformations might be silently ignored, and you'd only know by checking the logs.
*   **For Minimalists:** If you only have 1-2 simple jobs or touch your setup once every few months, don't feel forced into this setup. You can always stick with `nano` (or whatever you prefer) and just keep `journalctl -u spyweb -f` open in a separate terminal to check for errors after you save.

---

## 4. Accessing the UI (Securely)
By default, SpyWeb serves the admin dashboard on port **7979**. While you can open this port globally, we highly recommend one of the following secure access methods:

### Option 1: SSH Tunnel (Zero Public Exposure)
This is the **safest** method. You don't open any ports on the VPS; instead, you "tunnel" the traffic through your existing SSH connection.

```bash
# Run this on your LOCAL machine (Laptop/Desktop), not the VPS
ssh -L 7979:localhost:7979 user@your-vps-ip
```

Then, simply open `http://localhost:7979` in your local browser. The data travels through SSH, and the port remains closed to the outside world.

### Option 2: Nginx Reverse Proxy + Basic Auth
Use this if you want a custom domain and shared access. It adds a password prompt before anyone can see the dashboard.

```bash
# Install Nginx and password tools
sudo apt install nginx apache2-utils

# Create a password for your user
sudo htpasswd -c /etc/nginx/.htpasswd yourusername
```

Then create an Nginx config (e.g., `/etc/nginx/sites-available/spyweb`):

```nginx
server {
    listen 80;
    server_name your-domain.com;

    location / {
        auth_basic "SpyWeb Admin";
        auth_basic_user_file /etc/nginx/.htpasswd;
        proxy_pass http://127.0.0.1:7979;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

### Option 3: UFW Whitelist (Your IP Only)
If you have a static IP at home/office, you can tell the VPS firewall to only talk to you.

```bash
# Replace YOUR_HOME_IP with your actual public IP
sudo ufw allow from YOUR_HOME_IP to any port 7979
```

This blocks the entire world while letting you through seamlessly.

<!--
## Why no Docker?
You might be looking for a `Dockerfile` or a Docker Compose guide. Here is why you won't find one:

*   **It's an insult to SpyWeb's existence**: We worked hard to make a **7MB binary** with **zero dependencies**. Putting it in a 100MB Docker container is like putting a racing bike inside a shipping container to drive it across the street.
*   **Performance**: Running natively via `systemd` is the lightest possible way to run software. No container overhead, no virtual network layers.
*   **Developer Experience**: Docker volumes make editing Lua hooks and job configs a nightmare (permissions, laggy file-sync). Native deployment means your editor and your scraper are looking at the exact same files with zero friction.

If you really need isolation, use **systemd sandboxing** or run as a non-root user. Keep it lean.

> If you are managing a team of 50+ engineers sharing the same server which you might need docker, you probably don't need this guide to tell you how to containerize a 7MB binary.
-->
