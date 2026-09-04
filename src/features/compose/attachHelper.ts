export async function attachments_add_from_paths(
  paths: string[],
): Promise<{ name: string; mime: string; size: number; path: string }[]> {
  return paths.map((p) => {
    const name = p.split('/').pop() ?? 'file';
    return { name, mime: 'application/octet-stream', size: 0, path: p };
  });
}
