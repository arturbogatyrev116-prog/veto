"""Veto Messenger — remote server control via SSH.

Copy this file to veto_server.py and fill in your credentials.
veto_server.py is in .gitignore and will never be committed.
"""
import os, sys, time, paramiko

HOST     = "YOUR_VPS_IP"
USER     = "root"
PASSWORD = "YOUR_SSH_PASSWORD"   # or use SSH key auth (recommended)
COMPOSE  = "docker compose -f /opt/veto/docker-compose.prod.yml"
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
