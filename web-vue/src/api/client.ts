import { ofetch } from 'ofetch'

const apiClient = ofetch.create({
  baseURL: '/api',
  headers: {
    'Content-Type': 'application/json',
  },
  onResponseError({ response }) {
    const msg = response._data?.error || `请求失败 (${response.status})`
    console.error('[API Error]', msg)
    throw new Error(msg)
  },
})

/** GET 请求 — 后端返回裸数据（无 ApiResponse 包装） */
export async function get<T>(path: string, query?: Record<string, string | number>): Promise<T> {
  const params = query
    ? new URLSearchParams(
        Object.entries(query).map(([k, v]) => [k, String(v)]),
      ).toString()
    : ''
  const url = params ? `${path}?${params}` : path
  return await apiClient<T>(url)
}

/** POST 请求 — 后端返回 { success, ... }，失败时抛异常 */
export async function post<T = unknown>(path: string, body?: Record<string, unknown> | unknown[]): Promise<T> {
  const res = await apiClient<T & { success?: boolean; error?: string }>(path, {
    method: 'POST',
    body: body as BodyInit | Record<string, unknown> | undefined,
  })
  if (res && res.success === false) {
    throw new Error(res.error || '请求失败')
  }
  return res
}

/** DELETE 请求 — 后端返回 { success/deleted, ... }，失败时抛异常 */
export async function del<T = unknown>(path: string): Promise<T> {
  const res = await apiClient<T & { success?: boolean; deleted?: boolean; error?: string }>(
    path,
    { method: 'DELETE' },
  )
  if (res && (res.success === false || res.deleted === false)) {
    throw new Error(res.error || '删除失败')
  }
  return res
}

export default apiClient
