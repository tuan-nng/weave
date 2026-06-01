import { useParams } from "react-router";

export default function WorkspacePage() {
  const { id } = useParams<{ id: string }>();

  return (
    <div className="p-8">
      <h1 className="text-2xl font-bold">Workspace</h1>
      <p className="text-neutral-500 mt-2">ID: {id}</p>
    </div>
  );
}
