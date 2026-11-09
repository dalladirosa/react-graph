// not memoized: new props every render cost nothing extra here
export function Plain(props: any) {
  return <div {...props} />;
}
