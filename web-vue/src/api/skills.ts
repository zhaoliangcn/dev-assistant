import { get, post, del } from './client'

export interface SkillEntry {
  name: string
  description: string
  when_to_use?: string
  version?: string
  scope: string
  source?: string
}

export interface ListSkillsResponse {
  skills: SkillEntry[]
}

export interface InstallResponse {
  success: boolean
  installed: SkillEntry[]
}

export function listSkills(scope?: string): Promise<ListSkillsResponse> {
  return get<ListSkillsResponse>('/skills', scope ? { scope } : undefined)
}

export function installSkills(
  source: string,
  skillNames?: string[],
  scope?: string,
): Promise<InstallResponse> {
  return post<InstallResponse>('/skills/install', {
    source,
    skill_names: skillNames,
    scope,
  })
}

export function previewSkills(source: string): Promise<{ skills: SkillEntry[] }> {
  return post<{ skills: SkillEntry[] }>('/skills/preview', { source })
}

export function removeSkill(name: string, scope?: string): Promise<{ success: boolean }> {
  const query = scope ? `?scope=${encodeURIComponent(scope)}` : ''
  return del<{ success: boolean }>(`/skills/${encodeURIComponent(name)}${query}`)
}
