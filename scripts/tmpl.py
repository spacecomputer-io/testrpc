import argparse
import jinja2
import yaml

def render_template(template_path, values_path, num_nodes = 5):
    """
    Renders a Jinja2 template with variables from a YAML file and prints the output.

    Args:
        template_path (str): The path to the Jinja2 template file.
        values_path (str): The path to the YAML file with variables.
        --num-nodes (int): The number of nodes to create.
    """
    try:
        with open(values_path, 'r') as f:
            values = yaml.safe_load(f)

        values['num_nodes'] = num_nodes
        template_loader = jinja2.FileSystemLoader(searchpath="./")
        env = jinja2.Environment(loader=template_loader)
        template = env.get_template(template_path)

        rendered_output = template.render(values)
        print(rendered_output)
    except FileNotFoundError as e:
        print(f"Error: {e}")
    except jinja2.exceptions.TemplateError as e:
        print(f"Template rendering error: {e}")
    except yaml.YAMLError as e:
        print(f"YAML parsing error: {e}")

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Render a Jinja2 template with variables from a YAML file.")
    parser.add_argument("tmpl", help="The path to the Jinja2 template file.")
    parser.add_argument("values", help="The path to the YAML file with variables.")
    parser.add_argument("--num-nodes", type=int, default=5, help="The number of nodes to create.")

    args = parser.parse_args()
    render_template(args.tmpl, args.values, args.num_nodes)