export interface User {
  id: number;
  name: string;
  tags?: readonly string[];
}
export const isAdmin = (u: User): boolean => u.tags?.includes('admin') ?? false;
