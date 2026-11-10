// user travels App -> Layout -> Sidebar -> UserMenu -> Avatar; only Avatar reads it
export function Layout({ user, theme }: any) {
  return <Sidebar user={user} theme={theme} />;
}

export function Sidebar({ user, theme }: any) {
  return (
    <nav className={theme}>
      <UserMenu user={user} />
    </nav>
  );
}

export function UserMenu({ user }: any) {
  return <Avatar person={user} />;
}

export function Avatar({ person }: any) {
  return <img alt={person.name} />;
}
