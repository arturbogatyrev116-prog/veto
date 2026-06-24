const API_BASE = '/api/v1/admin';

let adminToken = localStorage.getItem('admin_token') || '';

export function setAdminToken(token) {
  adminToken = token;
  if (token) {
    localStorage.setItem('admin_token', token);
  } else {
    localStorage.removeItem('admin_token');
  }
}

export function getAdminToken() {
  return adminToken;
}

async function request(method, path, body = null) {
  const res = await fetch(`${API_BASE}${path}`, {
    method,
    headers: {
      'Authorization': `Bearer ${adminToken}`,
      'Content-Type': 'application/json',
    },
    body: body != null ? JSON.stringify(body) : null,
  });

  if (res.status === 401) throw new Error('unauthorized');
  if (res.status === 404) throw new Error('not found');
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(text || `HTTP ${res.status}`);
  }
  if (res.status === 204) return null;
  return res.json();
}

export const api = {
  listUsers:   ()                       => request('GET',    '/users'),
  createUser:  (username)               => request('POST',   '/users', { username }),
  blockUser:   (userId, reason)         => request('POST',   `/users/${userId}/block`,   reason ? { reason } : {}),
  unblockUser: (userId)                 => request('POST',   `/users/${userId}/unblock`),
  deleteUser:  (userId)                 => request('DELETE', `/users/${userId}`),
};
