export const isWindows = navigator.userAgent.includes('Windows');

function lastSeparator(path: string) {
  const slash = path.lastIndexOf('/');
  return isWindows ? Math.max(slash, path.lastIndexOf('\\')) : slash;
}

export function fileName(path: string) {
  return path.slice(lastSeparator(path) + 1) || path;
}

export function directoryPrefix(path: string) {
  return path.slice(0, lastSeparator(path) + 1);
}

export function inDirectory(directory: string, name: string) {
  const separator = lastSeparator(directory) === directory.length - 1 ? '' : isWindows ? '\\' : '/';
  return directory + separator + name;
}
